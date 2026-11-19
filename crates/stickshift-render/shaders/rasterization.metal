// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.
//
// Rasterization Shader - Phong Lighting Model + modern-look stack
//
// This shader implements traditional GPU rasterization with Blinn-Phong lighting,
// plus the "modern look" stack that lives entirely below the render seam:
//
//   * Linear-space lighting presented through an explicit sRGB encode + ACES
//     tonemap (colors correct, not washed out) — see `present_color`.
//   * A gradient sky drawn as a fullscreen pass (`sky_vertex_main` /
//     `sky_fragment_main`) instead of a flat clear, graded by each pixel's view
//     ray elevation so it is the fog's colour at the horizon and below it, and
//     carrying the **sun disc and its glow** at the direction the light actually
//     comes from.
//   * Distance fog that blends far geometry into the sky/horizon color, hiding
//     the streaming spawn edge (`apply_fog`).
//   * Real sun shadows: a depth-only pass from the sun
//     (`shadow_vertex_main`) renders every caster into a fitted shadow map
//     *before* the scene pass, and both lit pipelines look themselves up in it
//     with receiver-plane-biased, tent-filtered 4x4 PCF (`shadow_visibility`). The
//     fitting, stability and bias reasoning live in `src/shadow.rs`.
//   * Point and spot lights, clustered: the CPU bins each frame's lights into a
//     grid of screen tiles x depth slices, and a lit fragment shades only its
//     own cluster's list (`local_lights`). The binning lives in `src/lights.rs`.
//   * Emissive surfaces: a per-draw colour added after lighting, before fog.
//
// Bloom is not implemented: it needs several extra
// half-res offscreen targets + ping-pong pipelines that could not be validated
// in this environment. The MSAA / resolve wiring is all on the Rust side
// (`backend.rs`); this file only provides the per-fragment math. Both the
// untextured (`vertex_main`/`fragment_main`) and textured
// (`textured_vertex_main`/`textured_fragment_main`) pipelines run the same
// lighting + fog + shadow + present math, so the two pixel-hash reference scenes
// change together and are re-blessed together.

#include <metal_stdlib>
using namespace metal;

// ============================================================================
// Vertex Input/Output Structures
// ============================================================================

/// Vertex data input from CPU-side mesh buffers
struct VertexIn {
    float3 position [[attribute(0)]];  // World-space vertex position
    float3 normal [[attribute(1)]];    // Surface normal for lighting calculations
    float3 color [[attribute(2)]];     // Per-vertex color (RGB)
};

/// Vertex shader output / Fragment shader input
struct VertexOut {
    float4 position [[position]];      // Clip-space position (required for rasterization)
    float3 worldNormal;                // Interpolated normal for fragment shading
    float3 worldPosition;              // World-space position for lighting/fog/shadow
    float3 color;                      // Interpolated vertex color
    float  viewDepth;                  // Positive view-space distance for fog
    float3 emissive [[flat]];          // The draw's emissive colour (per draw, not interpolated)
};

// ============================================================================
// Uniform Structures
// ============================================================================

/// Per-draw uniforms passed from the CPU: the transforms, and the draw's
/// emissive colour. Mirrors the Rust `Uniforms`, 208 bytes.
struct Uniforms {
    float4x4 modelViewProjection;      // Combined model-view-projection matrix
    float4x4 model;                    // Model matrix (world pos for fog + shadow lookups; places casters)
    float4x4 normalMatrix;             // Carries normals: inverse-transpose of model's 3x3 (or model itself)
    float4   emissive;                 // xyz: linear emissive colour, added after lighting
};

/// Lighting parameters (directional light with Phong components) plus the
/// look parameters (fog, sky), the shadow-map parameters, the
/// camera block and the light matrix.
/// Mirrors the Rust `LightUniforms` **byte for byte** — 256 bytes.
///
/// LAYOUT WARNING: an MSL `float3` occupies 16 bytes, so each one below sits at a
/// 16-byte-aligned offset (0, 16, 48, 64, 112) and the two trailing `float4x4`s at
/// 128 and 192. The Rust side spells those pads out explicitly and asserts every offset;
/// a mismatch here does not fail to compile, it silently shades with the wrong
/// bytes (it has happened once — see the `REFERENCE_HASH` re-bless note).
struct Light {
    float3 direction;                  // Light direction in world space (points FROM the light)
    float3 color;                      // Light color (RGB)
    float ambientIntensity;            // Ambient light strength (constant illumination)
    float diffuseIntensity;            // Diffuse reflection strength (angle-dependent)
    float specularIntensity;           // Specular highlight strength (view-dependent)
    float shininess;                   // Phong exponent (controls highlight tightness)

    // --- Look params (sky, fog) ---
    float3 skyTopColor;                // Linear zenith color of the gradient sky
    float3 skyHorizonColor;            // Linear horizon color (fog blends toward this)
    float  fogDensity;                 // Exponential fog density (0 disables fog)
    float  fogStart;                   // View distance at which fog begins
    float  fogHeight;                  // World Y at/below which fog is full strength
    float  fogFalloff;                 // e-fold the fog thins over above fogHeight (0 = no falloff)

    // --- Shadow map (see src/shadow.rs for the reasoning) ---
    float  shadowDepthBias;            // Constant bias, map depth units (0.02 world / depth range)
    float  shadowReceiverSlopeCap;     // Cap on the receiver plane's per-texel depth gradient, map depth units
    float  shadowDistance;             // View depth the map covers; shadows fade out before it
    float  shadowStrength;             // 1 = shadowed fragments keep only sky fill, 0 = off

    // --- Camera block (the frame's camera, not its light) ---
    float3 cameraPosition;             // Camera world position -> view-dependent specular
    float4x4 inverseViewProjection;    // clip -> world, for the sky pass's view ray

    // --- Shadow light matrix ---
    float4x4 lightViewProjection;      // world -> shadow-map clip (fitted ortho * light view)
};

// ============================================================================
// Sun disc + glow tuning
// ============================================================================
//
// Angular sizes, stored as cosines so the fragment shader only ever needs one dot
// product. The real sun subtends ~0.53 degrees, which is a couple of pixels and
// reads as a stray dot, so the disc is drawn a little larger than life and the
// glow carries most of the impression of brightness.
//
// The glow extent is deliberately narrow (18 degrees). It has to be: the
// screen-space overlay reference renders with an identity view-projection, whose
// unprojection gives every pixel the constant view ray (0,0,1) — and the default
// sun sits 75 degrees off that, so a glow this tight contributes *exactly* zero
// there and cannot move OVERLAY_REFERENCE_HASH.

/// cos of the disc's solid core (~0.45 degrees).
constant float SUN_DISC_COS_INNER = 0.99997;
/// cos of the disc's outer rim (~0.95 degrees); the core fades to 0 by here.
constant float SUN_DISC_COS_OUTER = 0.99986;
/// cos of the glow's outer extent (~18 degrees).
constant float SUN_GLOW_COS = 0.95106;
/// How much brighter than the sun colour the disc's core is. Well over 1 so it
/// clips to white through the ACES tonemap, like looking at a real sun.
constant float SUN_DISC_GAIN = 12.0;
/// Peak brightness of the glow, as a fraction of the sun colour.
constant float SUN_GLOW_GAIN = 0.55;

/// How quickly the sky gives way from its horizon colour to its zenith colour
/// with height above the horizon, as the sine of the elevation (about 8°) at
/// which it is 63% of the way. A real sky is palest in a band along the horizon.
constant float SKY_HORIZON_BAND = 0.14;

// ============================================================================
// Color management: linear -> ACES tonemap -> sRGB encode
// ============================================================================

/// Narkowicz ACES filmic tonemap approximation, operating on linear HDR color.
inline float3 aces_tonemap(float3 x) {
    const float a = 2.51;
    const float b = 0.03;
    const float c = 2.43;
    const float d = 0.59;
    const float e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

/// Encode a linear color to sRGB (per-channel gamma). Both render targets are
/// *Unorm* (not `_sRGB`), so we encode explicitly here; the offscreen readback
/// then sees perceptually-correct bytes.
inline float3 linear_to_srgb(float3 c) {
    float3 lo = c * 12.92;
    float3 hi = 1.055 * pow(max(c, 1e-5), float3(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= 0.0031308);
}

/// Full present transform: tonemap a linear HDR color and sRGB-encode it for
/// output. Every lit/sky/composite fragment ends with this so colors are
/// consistent and not washed out.
inline float3 present_color(float3 linearHdr) {
    return linear_to_srgb(aces_tonemap(linearHdr));
}

// ============================================================================
// Distance fog
// ============================================================================

/// How much fog stands between the eye and a point `viewDist` away at height
/// `worldY`: `0` none, `1` all sky. Exponential-squared in distance,
/// **attenuated by world height**. `apply_fog` blends a colour by it; additive
/// particles fade by it instead, having no colour of their own to blend.
inline float fog_amount(float viewDist, float worldY, constant Light& light) {
    if (light.fogDensity <= 0.0) {
        return 0.0;
    }
    float d = max(viewDist - light.fogStart, 0.0) * light.fogDensity;
    float fog = 1.0 - exp(-d * d);

    // Height falloff: full strength at/below fogHeight, thinning upward.
    if (light.fogFalloff > 0.0) {
        fog *= exp(-max(worldY - light.fogHeight, 0.0) / light.fogFalloff);
    }

    return clamp(fog, 0.0, 1.0);
}

/// Exponential-squared distance fog blending `color` toward the horizon/sky
/// color, **attenuated by world height**. Returns linear color (present happens
/// afterward). Hides the streaming spawn edge by fading far geometry into the sky,
/// while the height falloff keeps the fog hugging the ground so tall geometry (the
/// skyline backdrop, high buildings) stays readable above it.
inline float3 apply_fog(float3 color, float viewDist, float worldY, constant Light& light) {
    if (light.fogDensity <= 0.0) {
        return color;
    }
    return mix(color, light.skyHorizonColor, fog_amount(viewDist, worldY, light));
}

// ============================================================================
// Shadow map
// ============================================================================
//
// Pass order: `shadow_vertex_main` renders every caster depth-only into the map
// first (its own render pass, stored — never memoryless — so it survives to be
// sampled), then the scene pass samples it here. All tuning constants and their
// justification live in `src/shadow.rs`; the per-frame values arrive in `Light`.

/// Fraction of `shadowDistance` over which shadows fade out (mirrors
/// `SHADOW_FADE_FRACTION` in `src/shadow.rs`).
constant float SHADOW_FADE_FRACTION = 0.2;

/// Depth-only caster input: the position is the only attribute the shadow pass
/// reads, at location 0 in both the `[pos,normal,color]` and `[pos,normal,uv]`
/// layouts (the pipeline's vertex descriptor supplies each layout's stride).
struct ShadowVertexIn {
    float3 position [[attribute(0)]];
};

/// Depth-only caster output: clip position only — there is no fragment stage.
struct ShadowVertexOut {
    float4 position [[position]];
};

/// The shadow pass's minimal vertex function: model -> world -> light clip. No
/// normal, colour or UV plumbing, and no fragment function at all.
vertex ShadowVertexOut shadow_vertex_main(ShadowVertexIn in [[stage_in]],
                                          constant Uniforms& uniforms [[buffer(1)]],
                                          constant Light& light [[buffer(2)]]) {
    ShadowVertexOut out;
    out.position = light.lightViewProjection * (uniforms.model * float4(in.position, 1.0));
    return out;
}

/// How much of the sun reaches `worldPos`: 1 = fully lit, 0 = fully shadowed.
///
/// Filtering: tent-weighted PCF over the 4x4 texels around the lookup — the
/// same weights as a 3x3 grid of bilinear taps one texel apart, i.e. a smooth
/// ~4-texel penumbra — done as explicit per-texel compares so each texel can be
/// compared at its own centre. Bias, per texel:
///
///  * **Receiver plane** — each texel is compared against the depth the
///    receiver's own triangle has *at that texel's centre*, extrapolated along the
///    triangle's plane. The plane comes from the screen-space derivatives of the
///    shadow-map position (exact for a planar triangle), so a lit surface never
///    shadows itself however steeply it faces away from the sun. The per-texel
///    gradient is capped at `shadowReceiverSlopeCap` so an edge-on receiver (whose
///    gradient is unbounded) cannot extrapolate absurdly.
///  * **`shadowDepthBias`** — 2 cm of world, for float error.
///
/// Anything outside the map, or past `shadowDistance`, is lit.
inline float shadow_visibility(float3 worldPos, float viewDepth, constant Light& light,
                               depth2d<float> shadowMap) {
    float4 lightClip = light.lightViewProjection * float4(worldPos, 1.0);
    float3 ndc = lightClip.xyz / lightClip.w;          // orthographic: w == 1
    // Shadow-map (u, v, depth). NDC +Y is up; texture v runs down.
    float3 q = float3(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);

    // Derivatives first, in uniform control flow (before any per-pixel branch).
    float3 qdx = dfdx(q);
    float3 qdy = dfdy(q);

    if (light.shadowStrength <= 0.0) {
        return 1.0;
    }
    if (any(q.xy < 0.0) || any(q.xy > 1.0) || q.z > 1.0 || q.z < 0.0) {
        return 1.0;
    }

    // Receiver plane in (u, v, depth): its normal is the cross of two in-plane
    // screen derivatives; depth's gradient over (u, v) follows from it.
    float size = float(shadowMap.get_width());
    float3 n = cross(qdx, qdy);
    float nLen = length(n);
    float2 depthPerUv = float2(0.0);
    if (nLen > 0.0) {
        // Edge-on to the light, n.z -> 0 and the gradient explodes: floor the
        // divisor, then cap the per-texel gradient.
        float nz = n.z >= 0.0 ? max(n.z, 1e-6 * nLen) : min(n.z, -1e-6 * nLen);
        depthPerUv = -n.xy / nz;
    }
    float capPerUv = light.shadowReceiverSlopeCap * size;
    depthPerUv = clamp(depthPerUv, -capPerUv, capPerUv);
    float reference = q.z - light.shadowDepthBias;

    // Texel-centre space: texel k's centre sits at k + 0.5 texels.
    float2 pos = q.xy * size - 0.5;
    float2 base = floor(pos);
    float2 f = pos - base;
    // 1D weights of three bilinear taps at -1, 0, +1 texels over texels
    // base-1 .. base+2 (each row sums to 3; the 2D total is 9).
    float wx[4] = { 1.0 - f.x, 1.0, 1.0, f.x };
    float wy[4] = { 1.0 - f.y, 1.0, 1.0, f.y };
    int maxIndex = int(size) - 1;

    float lit = 0.0;
    for (int j = 0; j < 4; ++j) {
        for (int i = 0; i < 4; ++i) {
            float2 texelIndex = base + float2(i - 1, j - 1);
            float2 centreUv = (texelIndex + 0.5) / size;
            float receiver = reference + dot(depthPerUv, centreUv - q.xy);
            uint2 coord = uint2(clamp(int2(texelIndex), int2(0), int2(maxIndex)));
            float occluder = shadowMap.read(coord);
            lit += wx[i] * wy[j] * (receiver <= occluder ? 1.0 : 0.0);
        }
    }
    lit *= (1.0 / 9.0);

    // Fade out over the last stretch of the fitted slab (inside the fog), so the
    // map's far edge never shows as a line.
    float fadeStart = light.shadowDistance * (1.0 - SHADOW_FADE_FRACTION);
    lit = mix(lit, 1.0, smoothstep(fadeStart, light.shadowDistance, viewDepth));

    return mix(1.0, lit, light.shadowStrength);
}

// ============================================================================
// Point and spot lights (clustered)
// ============================================================================
//
// The CPU bins the frame's lights into CLUSTER_TILES_X x CLUSTER_TILES_Y screen
// tiles x CLUSTER_SLICES exponential depth slices (`src/lights.rs`, which also
// holds the budget and why the binning is conservative). A fragment finds its
// cluster from its pixel position and view depth and loops over that cluster's
// list only. The constants and the cluster formula mirror `src/lights.rs`
// exactly: `cluster_of` there is this function's CPU twin, and the binning is
// tested against it.

constant uint CLUSTER_TILES_X = 16;
constant uint CLUSTER_TILES_Y = 9;
constant uint CLUSTER_SLICES = 24;

/// Below this distance a light stops brightening (mirrors `LIGHT_MIN_DISTANCE`
/// in `stickshift-render-api`): 0.1 world units, squared.
constant float LIGHT_MIN_DISTANCE_SQ = 0.01;

/// One point or spot light. Mirrors the Rust `GpuLight`: four float4s, 64 bytes.
struct LocalLight {
    float4 positionInvRangeSq;         // xyz world position, w = 1 / range^2
    float4 radiance;                   // xyz colour * intensity (linear)
    float4 spotDirection;              // xyz unit direction the light travels, w = cos(outer)
    float4 spotParams;                 // x = cos(inner), y = 1 for a spot, 0 for a point
};

/// The frame's cluster header. Mirrors the Rust `ClusterInfo`, 32 bytes.
struct ClusterInfo {
    float4 tileAndSlice;               // tiles per pixel (x, y), slice scale, slice bias
    uint4  counts;                     // x = lights uploaded this frame (0: skip the loop)
};

/// The cluster a fragment at `pixel` (origin top-left) and `viewDepth` is in.
inline uint cluster_index(float2 pixel, float viewDepth, constant ClusterInfo& info) {
    uint tx = min(uint(max(pixel.x * info.tileAndSlice.x, 0.0)), CLUSTER_TILES_X - 1);
    uint ty = min(uint(max(pixel.y * info.tileAndSlice.y, 0.0)), CLUSTER_TILES_Y - 1);
    float s = log2(max(viewDepth, 1e-4)) * info.tileAndSlice.z + info.tileAndSlice.w;
    uint tz = uint(clamp(s, 0.0, float(CLUSTER_SLICES - 1)));
    return (tz * CLUSTER_TILES_Y + ty) * CLUSTER_TILES_X + tx;
}

/// Light from this fragment's cluster's point and spot lights, in LINEAR space,
/// shaded with the same Blinn-Phong response as the sun (its specular strength
/// and shininess) and already multiplied by `albedo`. Falloff is the inverse
/// square, windowed to exactly zero at each light's range; a spot's cone fades
/// between its inner and outer angles. See `Light` in `stickshift-render-api`
/// for the curve, which `Light::falloff` / `Light::cone` compute on the CPU.
inline float3 local_lights(float3 albedo, float3 normal, float3 worldPos, float3 viewDir,
                           float2 pixel, float viewDepth, constant Light& light,
                           constant ClusterInfo& info,
                           device const LocalLight* lights,
                           device const uint2* clusters,
                           device const ushort* lightIndices) {
    float3 sum = float3(0.0);
    if (info.counts.x == 0) {
        return sum;
    }
    uint2 run = clusters[cluster_index(pixel, viewDepth, info)];
    for (uint i = 0; i < run.y; ++i) {
        LocalLight l = lights[lightIndices[run.x + i]];
        float3 toLight = l.positionInvRangeSq.xyz - worldPos;
        float d2 = dot(toLight, toLight);
        float ratio2 = d2 * l.positionInvRangeSq.w;      // (d / range)^2
        if (ratio2 >= 1.0) {
            continue;
        }
        float window = 1.0 - ratio2 * ratio2;            // 1 - (d / range)^4
        float attenuation = window * window / max(d2, LIGHT_MIN_DISTANCE_SQ);
        float3 L = toLight * rsqrt(max(d2, 1e-12));
        if (l.spotParams.y > 0.5) {
            attenuation *= smoothstep(l.spotDirection.w, l.spotParams.x,
                                      dot(-L, l.spotDirection.xyz));
        }
        float diff = max(dot(normal, L), 0.0);
        float3 halfDir = normalize(L + viewDir);
        float spec = pow(max(dot(normal, halfDir), 0.0), light.shininess) * step(1e-4, diff);
        sum += (diff + light.specularIntensity * spec) * attenuation * l.radiance.xyz;
    }
    return sum * albedo;
}

// ============================================================================
// Vertex Shader
// ============================================================================

/// Transform vertices to clip space and pass data to fragment shader
vertex VertexOut vertex_main(VertexIn in [[stage_in]],
                             constant Uniforms& uniforms [[buffer(1)]]) {
    VertexOut out;

    float4 clip = uniforms.modelViewProjection * float4(in.position, 1.0);
    out.position = clip;

    float4 world = uniforms.model * float4(in.position, 1.0);
    out.worldPosition = world.xyz;
    // Normal transformed by the model matrix (uniform scale assumed for boxes).
    out.worldNormal = (uniforms.normalMatrix * float4(in.normal, 0.0)).xyz;
    out.color = in.color;
    // Clip.w is the positive view-space depth for our right-handed projection.
    out.viewDepth = clip.w;
    out.emissive = uniforms.emissive.xyz;

    return out;
}

// ============================================================================
// Fragment Shader
// ============================================================================

/// Blinn-Phong lit color in LINEAR space, shared by the lit fragment shaders:
/// the sun, the cluster's point and spot lights, and the draw's emission, then
/// fog. `shadow` (from `shadow_visibility`) scales the sun's direct light only;
/// the ambient sky fill and the local lights reach shadowed surfaces unchanged.
inline float3 lit_linear(float3 albedo, float3 emissive, float3 worldNormal, float3 worldPos,
                         float viewDepth, float2 pixel, float shadow, constant Light& light,
                         constant ClusterInfo& clusterInfo,
                         device const LocalLight* lights,
                         device const uint2* clusters,
                         device const ushort* lightIndices) {
    float3 normal = normalize(worldNormal);
    float3 lightDir = normalize(-light.direction);

    float3 ambient = light.ambientIntensity * light.color;
    float diff = max(dot(normal, lightDir), 0.0);
    float3 diffuse = light.diffuseIntensity * diff * light.color;

    // View-dependent specular: the eye position comes across the seam
    // each frame, so highlights track the camera. This used to be a constant
    // float3(0,0,1), which pinned every highlight to a fixed screen direction —
    // most visible on car bodywork under a low sun, where the highlight should
    // sweep as you drive past it and simply did not.
    float3 viewDir = normalize(light.cameraPosition - worldPos);
    float3 halfDir = normalize(lightDir + viewDir);
    float spec = pow(max(dot(normal, halfDir), 0.0), light.shininess);
    // Only lit faces can have a highlight. With a real view vector a face turned
    // away from the sun but toward the camera would otherwise pick up a spurious
    // one.
    spec *= step(1e-4, diff);
    // Scaled by the sun's intensity, like its diffuse light and its disc, so a
    // dimmer sun (or the moon standing in for it) has dimmer highlights.
    float3 specular = light.specularIntensity * light.diffuseIntensity * spec * light.color;

    float3 lit = (ambient + (diffuse + specular) * shadow) * albedo;
    // With no lights and no emission both terms are exactly zero, so a frame
    // without them shades bit-for-bit as it did before they existed.
    lit += local_lights(albedo, normal, worldPos, viewDir, pixel, viewDepth, light,
                        clusterInfo, lights, clusters, lightIndices)
         + emissive;
    return apply_fog(lit, viewDepth, worldPos.y, light);
}

/// Calculate per-pixel lighting using Blinn-Phong shading model (untextured).
fragment float4 fragment_main(VertexOut in [[stage_in]],
                              constant Light& light [[buffer(0)]],
                              constant ClusterInfo& clusterInfo [[buffer(3)]],
                              device const LocalLight* lights [[buffer(4)]],
                              device const uint2* clusters [[buffer(5)]],
                              device const ushort* lightIndices [[buffer(6)]],
                              depth2d<float> shadowMap [[texture(1)]]) {
    float shadow = shadow_visibility(in.worldPosition, in.viewDepth, light, shadowMap);
    float3 lit = lit_linear(in.color, in.emissive, in.worldNormal, in.worldPosition,
                            in.viewDepth, in.position.xy, shadow, light,
                            clusterInfo, lights, clusters, lightIndices);
    return float4(present_color(lit), 1.0);
}

// ============================================================================
// Gradient sky
// ============================================================================
//
// A fullscreen triangle drawn first (depth test disabled) that paints a vertical
// gradient from `skyHorizonColor` up to `skyTopColor`, replacing the flat clear.
// Far geometry fogs toward `skyHorizonColor`, so the horizon reads seamlessly.
//
// The **sun** is added to it: a small disc with a soft halo, placed at
// `-light.direction` — the very vector `lit_linear` shades with, so turning the
// sun moves the disc and the lighting together and they cannot drift apart. It
// stays a single fullscreen pass with no new geometry: the per-pixel view ray is
// unprojected from the NDC the vertex stage already interpolates, and "is this
// pixel the sun?" is one dot product plus two smoothsteps.

struct SkyOut {
    float4 position [[position]];
    float2 ndc;                        // -1..1 clip position, unprojected to the view ray
};

/// Fullscreen triangle (three verts covering the viewport, no vertex buffer).
vertex SkyOut sky_vertex_main(uint vid [[vertex_id]]) {
    // Oversized triangle covering the whole clip volume.
    float2 pos[3] = { float2(-1.0, -3.0), float2(-1.0, 1.0), float2(3.0, 1.0) };
    SkyOut out;
    out.position = float4(pos[vid], 1.0, 1.0);   // z=1 -> far plane, behind everything
    out.ndc = pos[vid];
    return out;
}

/// World-space view ray through a fullscreen-pass pixel, from its NDC.
///
/// The sky owns no geometry, so it recovers the ray by unprojecting the pixel at
/// both ends of the depth range through the camera's inverse view-projection and
/// taking the direction between them. That works for any projection the seam
/// pushes, and degenerates safely: with an identity view-projection (the backend's
/// state before the first camera push) it yields a constant `(0, 0, 1)` rather
/// than NaNs.
inline float3 sky_view_ray(float2 ndc, constant Light& light) {
    float4 nearH = light.inverseViewProjection * float4(ndc, 0.0, 1.0);
    float4 farH  = light.inverseViewProjection * float4(ndc, 1.0, 1.0);
    return normalize(farH.xyz / farH.w - nearH.xyz / nearH.w);
}

/// Gradient sky, graded by elevation, **plus the sun disc and its glow**, tonemapped
/// + sRGB-encoded like the rest of the scene — so a blazing disc clips to white
/// through the same ACES curve as everything else instead of blowing out.
fragment float4 sky_fragment_main(SkyOut in [[stage_in]],
                                  constant Light& light [[buffer(0)]]) {
    float3 rayDir = sky_view_ray(in.ndc, light);

    // The gradient follows the view ray's elevation, not the pixel's height on
    // screen. So the horizon colour lies along the real horizon, and below it,
    // which is the colour distance fog fades far geometry to: wherever the world
    // ends, it meets sky of its own colour, with no seam, whatever the camera's
    // pitch. Above the horizon the sky deepens to the zenith colour over a band
    // of a few degrees (`SKY_HORIZON_BAND`).
    float t = 1.0 - exp(-max(rayDir.y, 0.0) / SKY_HORIZON_BAND);
    float3 sky = mix(light.skyHorizonColor, light.skyTopColor, t);

    // The sun: one dot product between this pixel's view ray and the direction the
    // sunlight arrives *from*. `light.direction` is the direction it travels, so
    // the sun is at its negation — the same single source of truth the shading
    // uses, which is what keeps the disc and the highlights on the same sun.
    float cosToSun = dot(rayDir, normalize(-light.direction));

    // Core disc, antialiased over the rim by the smoothstep, and a halo that falls
    // off into the gradient (cubed so it stays tight near the disc and vanishes
    // gently). Both are exactly 0 outside their extents, so a frame whose sun is
    // off-screen is bit-identical to the plain gradient.
    float disc = smoothstep(SUN_DISC_COS_OUTER, SUN_DISC_COS_INNER, cosToSun);
    float glow = smoothstep(SUN_GLOW_COS, 1.0, cosToSun);
    glow = glow * glow * glow;

    // Scaled by the sun's own intensity so a dimmer sun also has a dimmer disc.
    float3 sun = light.color * light.diffuseIntensity
               * (disc * SUN_DISC_GAIN + glow * SUN_GLOW_GAIN);

    return float4(present_color(sky + sun), 1.0);
}

// ============================================================================
// Textured pipeline
// ============================================================================
//
// A second pipeline reading the `[pos,normal,uv]` layout. It samples the bound
// base-color texture at the interpolated UV (trilinear, mipmapped) and runs the
// same lighting + fog + shadow-map + present math as the untextured path.

/// Textured vertex input: position, normal, and UV (no per-vertex color).
struct TexturedVertexIn {
    float3 position [[attribute(0)]];
    float3 normal [[attribute(1)]];
    float2 uv [[attribute(2)]];
};

/// Textured vertex output / fragment input.
struct TexturedVertexOut {
    float4 position [[position]];
    float3 worldNormal;
    float3 worldPosition;
    float2 uv;
    float  viewDepth;
    float3 emissive [[flat]];
};

/// Per-draw material parameters for the textured pipeline (base-color factor).
/// Mirrors the Rust `MaterialUniforms` (`[[buffer(2)]]`), 16 bytes.
struct MaterialUniforms {
    float4 baseColorFactor;            // Linear RGBA, multiplied with the texture
};

/// Textured vertex shader: transform to clip space, pass world data + UV through.
vertex TexturedVertexOut textured_vertex_main(TexturedVertexIn in [[stage_in]],
                                              constant Uniforms& uniforms [[buffer(1)]]) {
    TexturedVertexOut out;
    float4 clip = uniforms.modelViewProjection * float4(in.position, 1.0);
    out.position = clip;
    float4 world = uniforms.model * float4(in.position, 1.0);
    out.worldPosition = world.xyz;
    out.worldNormal = (uniforms.normalMatrix * float4(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.viewDepth = clip.w;
    out.emissive = uniforms.emissive.xyz;
    return out;
}

/// Textured fragment shader: sample the bound base-color texture (trilinear) at
/// the UV, modulate by the material base-color factor, and light/fog/present.
fragment float4 textured_fragment_main(TexturedVertexOut in [[stage_in]],
                                       constant Light& light [[buffer(0)]],
                                       constant MaterialUniforms& material [[buffer(2)]],
                                       constant ClusterInfo& clusterInfo [[buffer(3)]],
                                       device const LocalLight* lights [[buffer(4)]],
                                       device const uint2* clusters [[buffer(5)]],
                                       device const ushort* lightIndices [[buffer(6)]],
                                       texture2d<float> baseColor [[texture(0)]],
                                       sampler baseColorSampler [[sampler(0)]],
                                       depth2d<float> shadowMap [[texture(1)]]) {
    float4 sampled = baseColor.sample(baseColorSampler, in.uv);
    float3 albedo = sampled.rgb * material.baseColorFactor.rgb;

    float shadow = shadow_visibility(in.worldPosition, in.viewDepth, light, shadowMap);
    float3 lit = lit_linear(albedo, in.emissive, in.worldNormal, in.worldPosition,
                            in.viewDepth, in.position.xy, shadow, light,
                            clusterInfo, lights, clusters, lightIndices);
    return float4(present_color(lit), sampled.a * material.baseColorFactor.a);
}

// ============================================================================
// 2D overlay / HUD pass
// ============================================================================
//
// Screen-space quads drawn AFTER the 3D scene: orthographic, no depth test or
// write, alpha blended. Positions arrive in pixels with the origin at the
// drawable's top-left; the vertex shader maps them to NDC using the viewport
// size. `mode` selects how the texel is used (see OverlayFill on the Rust side):
//   0 = solid   (ignore the texture, use the vertex color)
//   1 = textured(sample RGBA, multiply by the vertex color)
//   2 = SDF     (distance in .r -> crisp coverage, tinted by the vertex color)
//   3 = textured from an sRGB texture (re-encoded, so it matches mode 1's bytes)

/// Per-vertex overlay data: pixel position, atlas UV, RGBA tint, fill mode.
struct OverlayVertexIn {
    float2 position [[attribute(0)]];
    float2 uv       [[attribute(1)]];
    float4 color    [[attribute(2)]];
    float  mode     [[attribute(3)]];
};

struct OverlayVertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
    float  mode;
};

/// Per-pass overlay uniforms: the drawable size used for the ortho mapping.
struct OverlayUniforms {
    float2 viewportSize;               // Drawable size in pixels
};

/// Overlay vertex shader: pixels (origin top-left, +Y down) -> NDC.
vertex OverlayVertexOut overlay_vertex_main(OverlayVertexIn in [[stage_in]],
                                            constant OverlayUniforms& uniforms [[buffer(1)]]) {
    OverlayVertexOut out;
    // Pixel -> [0,1] -> NDC, flipping Y because NDC is +Y up.
    float2 unit = in.position / uniforms.viewportSize;
    out.position = float4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.mode = in.mode;
    return out;
}

/// Overlay fragment shader. Output is already display-referred (the overlay is
/// authored in display space and composites over the tonemapped scene), so it
/// does NOT run the scene's tonemap/encode path.
fragment float4 overlay_fragment_main(OverlayVertexOut in [[stage_in]],
                                      texture2d<float> atlas [[texture(0)]],
                                      sampler atlasSampler [[sampler(0)]]) {
    if (in.mode < 0.5) {
        // Solid.
        return in.color;
    }

    float4 sampled = atlas.sample(atlasSampler, in.uv);

    if (in.mode < 1.5) {
        // Straight textured.
        return sampled * in.color;
    }

    if (in.mode > 2.5) {
        // Textured from an sRGB texture: the sampler decoded it to linear, but
        // the overlay is display-referred, so re-encode to the stored colours.
        return float4(linear_to_srgb(sampled.rgb), sampled.a) * in.color;
    }

    // SDF text: the atlas stores signed distance in .r, 0.5 being the edge.
    // Derive a screen-space-consistent antialiasing width from the distance
    // field's own gradient so glyphs stay crisp at any scale.
    float dist = sampled.r;
    float width = max(fwidth(dist), 1e-4);
    float coverage = smoothstep(0.5 - width, 0.5 + width, dist);
    return float4(in.color.rgb, in.color.a * coverage);
}

// ============================================================================
// Particles
// ============================================================================
//
// Particles live only here, in one ring buffer of slots the CPU never reads
// back (src/particles.rs has the CPU half: what to spawn, and which slots are
// live). Each frame runs `particle_update` over the particles already there,
// then `particle_spawn` over the frame's new ones, in a compute pass before the
// scene pass; the scene pass then draws the live slots as instanced quads,
// after the geometry and before the overlay.

/// One particle: 48 bytes. A dead particle has `age >= lifetime`.
struct Particle {
    float4 positionAge;                // xyz world position, w age in seconds
    float4 velocityLife;               // xyz velocity, w lifetime in seconds
    uint4  meta;                       // x its emitter's slot, y its heading (float bits)
};

/// One emitter's params. Mirrors the Rust `EmitterUniforms`, 128 bytes.
struct EmitterParams {
    float4 colorStart;                 // linear RGBA at the start of life
    float4 colorEnd;                   // ... and at the end
    float4 gravityDrag;                // xyz gravity, w drag
    float4 look;                       // size start, size end, stretch, blend (0 add, 1 alpha)
    float4 floorParams;                // height, bounce, friction, 1 if there is a floor
    float4 directionSpread;            // xyz unit direction (emitter frame), w cos(spread)
    float4 lifetimeSpeed;              // lifetime min, max, speed min, max
    float4 radiusInheritFacing;        // spawn radius, share of emitter velocity, facing (0 camera, 1 flat), -
};

/// A run of particles to spawn. Mirrors the Rust `SpawnBatch`, 80 bytes.
struct SpawnBatch {
    float4 from;                       // xyz path start, w seconds into the frame it starts
    float4 to;                         // xyz path end, w the span's length (0 for a burst)
    float4 rotation;                   // the emitter's rotation, quaternion xyzw
    float4 velocity;                   // xyz the emitter's velocity
    uint4  meta;                       // emitter slot, first spawn index, count, seed
};

/// The compute kernels' per-frame values. Mirrors the Rust `ParticleStep`.
struct ParticleStep {
    float4 shiftDt;                    // xyz shift for the particles already there, w seconds
    uint4  window;                     // their first slot, count; ring size; spawn count
    uint4  spawn;                      // first spawn slot, batch count
};

/// The draw's per-frame values. Mirrors the Rust `ParticleView`.
struct ParticleView {
    float4x4 viewProjection;           // world -> clip
    float4   camera;                   // xyz camera position, w the blend drawn
    uint4    window;                   // first live slot, count, ring size
};

/// A 32-bit integer hash (PCG): the particles' random numbers come from hashing
/// a batch's seed with the particle's index, so the same batch spawns the same
/// particles.
inline uint pcg_hash(uint v) {
    uint state = v * 747796405u + 2891336453u;
    uint word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

/// The next random number in `[0, 1)` from `state`.
inline float next_unit(thread uint& state) {
    state = pcg_hash(state);
    return float(state >> 8) * (1.0 / 16777216.0);
}

/// `v` turned by the unit quaternion `q` (xyzw).
inline float3 rotate_by(float4 q, float3 v) {
    float3 t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

/// Move a particle on `dt` seconds: drag, gravity, then the floor.
inline void integrate(thread float3& p, thread float3& v, float dt, constant EmitterParams& e) {
    if (dt <= 0.0) {
        return;
    }
    v *= exp(-e.gravityDrag.w * dt);
    v += e.gravityDrag.xyz * dt;
    p += v * dt;
    float floorY = e.floorParams.x;
    if (e.floorParams.w > 0.5 && p.y < floorY && v.y < 0.0) {
        p.y = floorY + (floorY - p.y) * e.floorParams.y;
        v.y = -v.y * e.floorParams.y;
        v.xz *= 1.0 - e.floorParams.z;
    }
}

/// Move every particle already in the ring on by the frame's time, shifted
/// with the world. Dead particles are left as they are.
kernel void particle_update(device Particle* particles [[buffer(0)]],
                            constant EmitterParams* emitters [[buffer(1)]],
                            constant ParticleStep& step [[buffer(2)]],
                            uint id [[thread_position_in_grid]]) {
    if (id >= step.window.y) {
        return;
    }
    uint slot = (step.window.x + id) % step.window.z;
    Particle part = particles[slot];
    if (part.positionAge.w >= part.velocityLife.w) {
        return;
    }
    float3 p = part.positionAge.xyz + step.shiftDt.xyz;
    float3 v = part.velocityLife.xyz;
    integrate(p, v, step.shiftDt.w, emitters[part.meta.x]);
    part.positionAge = float4(p, part.positionAge.w + step.shiftDt.w);
    part.velocityLife.xyz = v;
    particles[slot] = part;
}

/// Spawn the frame's new particles into the next slots of the ring: each
/// leaves from along its batch's path, within its emitter's radius and cone,
/// and is moved on by the part of the frame after it was born.
kernel void particle_spawn(device Particle* particles [[buffer(0)]],
                           constant EmitterParams* emitters [[buffer(1)]],
                           constant ParticleStep& step [[buffer(2)]],
                           constant SpawnBatch* batches [[buffer(3)]],
                           uint id [[thread_position_in_grid]]) {
    if (id >= step.window.w) {
        return;
    }
    // The batch this particle is in: the last one starting at or before it.
    uint lo = 0;
    uint hi = step.spawn.y;
    while (hi - lo > 1) {
        uint mid = (lo + hi) / 2;
        if (batches[mid].meta.y <= id) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    constant SpawnBatch& batch = batches[lo];
    constant EmitterParams& e = emitters[batch.meta.x];
    uint index = id - batch.meta.y;
    uint state = pcg_hash(batch.meta.w ^ pcg_hash(index));

    // When and where: spread evenly over the span and along the path.
    float along = (float(index) + next_unit(state)) / float(max(batch.meta.z, 1u));
    float born = batch.from.w + along * batch.to.w;
    float3 p = mix(batch.from.xyz, batch.to.xyz, along);
    float cosBall = next_unit(state) * 2.0 - 1.0;
    float sinBall = sqrt(max(0.0, 1.0 - cosBall * cosBall));
    float ballTurn = next_unit(state) * 6.28318530718;
    float3 ball = float3(sinBall * cos(ballTurn), cosBall, sinBall * sin(ballTurn));
    p += ball * pow(next_unit(state), 1.0 / 3.0) * e.radiusInheritFacing.x;

    // Which way: evenly over the cone around the emitter's direction.
    float3 axis = e.directionSpread.xyz;
    float cosTheta = mix(1.0, e.directionSpread.w, next_unit(state));
    float sinTheta = sqrt(max(0.0, 1.0 - cosTheta * cosTheta));
    float turn = next_unit(state) * 6.28318530718;
    float3 helper = abs(axis.y) < 0.99 ? float3(0.0, 1.0, 0.0) : float3(1.0, 0.0, 0.0);
    float3 across = normalize(cross(helper, axis));
    float3 dir = axis * cosTheta + (across * cos(turn) + cross(axis, across) * sin(turn)) * sinTheta;
    dir = rotate_by(batch.rotation, dir);

    float speed = mix(e.lifetimeSpeed.z, e.lifetimeSpeed.w, next_unit(state));
    float3 v = dir * speed + batch.velocity.xyz * e.radiusInheritFacing.y;
    float life = mix(e.lifetimeSpeed.x, e.lifetimeSpeed.y, next_unit(state));
    float age = max(step.shiftDt.w - born, 0.0);
    integrate(p, v, age, e);

    // The heading the emitter pointed along, for a flat particle: the way
    // `(sin h, 0, -cos h)` points, as a turn from `-Z` toward `+X`.
    float3 pointing = rotate_by(batch.rotation, axis);
    float heading = length(pointing.xz) > 1e-6 ? atan2(pointing.x, -pointing.z) : 0.0;

    Particle part;
    part.positionAge = float4(p, age);
    part.velocityLife = float4(v, life);
    part.meta = uint4(batch.meta.x, as_type<uint>(heading), 0, 0);
    particles[(step.spawn.x + id) % step.window.z] = part;
}

struct ParticleOut {
    float4 position [[position]];
    float4 color;                      // linear RGBA at this point of its life
    float2 corner;                     // -1..1 across the quad, for its soft edge
    float  viewDist;                   // for fog
    float  worldY;                     // for fog's height falloff
};

/// The six corners of a quad, as two triangles.
constant float2 PARTICLE_CORNERS[6] = {
    float2(-1.0, -1.0), float2(1.0, -1.0), float2(1.0, 1.0),
    float2(-1.0, -1.0), float2(1.0, 1.0), float2(-1.0, 1.0),
};

/// One corner of one live particle's quad: facing the camera, or for an emitter
/// with a stretch, a streak along its motion (as seen from the camera) whose
/// head is the particle, or for a flat emitter, a square lying in the
/// horizontal plane turned to the heading it was made with. A dead particle, or one drawn by the other blend, is
/// put outside the clip volume so its quad draws nothing.
vertex ParticleOut particle_vertex_main(uint vid [[vertex_id]],
                                        uint iid [[instance_id]],
                                        const device Particle* particles [[buffer(0)]],
                                        constant EmitterParams* emitters [[buffer(1)]],
                                        constant ParticleView& view [[buffer(2)]]) {
    ParticleOut out;
    out.position = float4(0.0, 0.0, 2.0, 1.0);
    out.color = float4(0.0);
    out.corner = float2(0.0);
    out.viewDist = 0.0;
    out.worldY = 0.0;

    Particle part = particles[(view.window.x + iid) % view.window.z];
    float age = part.positionAge.w;
    float life = part.velocityLife.w;
    constant EmitterParams& e = emitters[part.meta.x];
    if (age >= life || abs(e.look.w - view.camera.w) > 0.5) {
        return out;
    }
    float t = saturate(age / life);
    float size = mix(e.look.x, e.look.y, t);

    float3 p = part.positionAge.xyz;
    float3 toEye = view.camera.xyz - p;
    float eyeDist = length(toEye);
    float3 d = eyeDist > 1e-5 ? toEye / eyeDist : float3(0.0, 0.0, 1.0);
    float3 v = part.velocityLife.xyz;
    float3 crossing = v - d * dot(v, d);
    float crossingSpeed = length(crossing);

    float3 along;
    float3 side;
    float length_ = size;
    float3 centre = p;
    if (e.radiusInheritFacing.z > 0.5) {
        float heading = as_type<float>(part.meta.y);
        along = float3(sin(heading), 0.0, -cos(heading));
        side = float3(cos(heading), 0.0, sin(heading));
    } else if (e.look.z > 0.0 && crossingSpeed > 1e-4) {
        along = crossing / crossingSpeed;
        side = normalize(cross(d, along));
        length_ = size + crossingSpeed * e.look.z;
        centre = p - along * (length_ - size) * 0.5;
    } else {
        float3 up = abs(d.y) < 0.99 ? float3(0.0, 1.0, 0.0) : float3(1.0, 0.0, 0.0);
        side = normalize(cross(up, d));
        along = cross(d, side);
    }

    float2 corner = PARTICLE_CORNERS[vid];
    float3 world = centre + along * (corner.x * length_ * 0.5) + side * (corner.y * size * 0.5);
    out.position = view.viewProjection * float4(world, 1.0);
    out.color = mix(e.colorStart, e.colorEnd, t);
    out.corner = corner;
    out.viewDist = length(world - view.camera.xyz);
    out.worldY = world.y;
    return out;
}

/// How much of a particle's quad is lit at `corner`: full at its middle,
/// fading to nothing at its edges.
inline float particle_shape(float2 corner) {
    return saturate(1.0 - corner.x * corner.x) * saturate(1.0 - corner.y * corner.y);
}

/// An additive particle: its colour, presented, scaled by its alpha, its shape
/// and how much fog is in front of it, added onto the frame (the blend adds
/// `rgb` and keeps the frame's alpha).
fragment float4 particle_additive_fragment(ParticleOut in [[stage_in]],
                                           constant Light& light [[buffer(0)]]) {
    float fade = in.color.a * particle_shape(in.corner)
        * (1.0 - fog_amount(in.viewDist, in.worldY, light));
    return float4(present_color(in.color.rgb) * fade, 0.0);
}

/// An alpha particle: its colour fogged like the scene and presented, covering
/// the frame by its alpha and shape.
fragment float4 particle_alpha_fragment(ParticleOut in [[stage_in]],
                                        constant Light& light [[buffer(0)]]) {
    float3 color = apply_fog(in.color.rgb, in.viewDist, in.worldY, light);
    return float4(present_color(color), in.color.a * particle_shape(in.corner));
}
