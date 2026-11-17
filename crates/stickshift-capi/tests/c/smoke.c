/*
 * The C API smoke test: a C program built against stickshift.h and libstickshift.
 *
 *   smoke [--null] <model.gltf>
 *
 * Exercises the whole lifecycle through the header (context, mesh, borrowed
 * texture data, a model loaded off-thread with a completion callback and drawn
 * whole and by part, a frame with a read-back, events, stats, the math helpers)
 * and checks the pixels: something was drawn on Metal, zeros on the null
 * backend. Exits 0 on success, 1 on failure, and 0 with a message when Metal is
 * requested but no device exists.
 *
 * Run by scripts/test-c-api.sh (and CI).
 *
 * Copyright (c) 2026 Parsa Bagheri. Released under the Apache-2.0 License.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "stickshift.h"

#define CHECK(expr, expected)                                                         \
    do {                                                                              \
        StickShiftResult check_result_ = (expr);                                           \
        if (check_result_ != (expected)) {                                            \
            fprintf(stderr, "%s:%d: %s returned %s, expected %s (%s)\n", __FILE__,    \
                    __LINE__, #expr, stickshift_result_name(check_result_), #expected,     \
                    stickshift_last_error_message());                                      \
            return 1;                                                                 \
        }                                                                             \
    } while (0)

#define REQUIRE(cond)                                                                 \
    do {                                                                              \
        if (!(cond)) {                                                                \
            fprintf(stderr, "%s:%d: requirement failed: %s\n", __FILE__, __LINE__,    \
                    #cond);                                                           \
            return 1;                                                                 \
        }                                                                             \
    } while (0)

enum { WIDTH = 64, HEIGHT = 48 };

static int releases = 0;
static void count_release(void *user, const void *ptr, size_t len) {
    (void)user;
    (void)ptr;
    (void)len;
    releases++;
}

static int model_events = 0;
static StickShiftResult model_result = STICKSHIFT_OK;
static void on_model(void *user, const StickShiftEvent *event) {
    (void)user;
    model_events++;
    model_result = event->kind == STICKSHIFT_EVENT_RESOURCE_READY ? STICKSHIFT_OK : event->result;
}

static StickShiftData copied(const void *ptr, size_t len) {
    StickShiftData data = {ptr, len, NULL, NULL};
    return data;
}

static int near(float a, float b) {
    float d = a - b;
    return d < 1e-6f && d > -1e-6f;
}

static int near3(StickShiftVec3 a, StickShiftVec3 b) {
    return near(a.x, b.x) && near(a.y, b.y) && near(a.z, b.z);
}

/* The glam helpers (pure, no context). The values chosen are exact in f32, so they compare with ==;
 * the bit-for-bit checks against glam are the Rust tests in src/math.rs. */
static int check_math(void) {
    StickShiftVec3 a = {1, 2, 3}, b = {4, 5, 6}, zero = {0, 0, 0};
    StickShiftVec3 x = {1, 0, 0}, y = {0, 1, 0}, ahead = {0, 0, -1};
    REQUIRE(stickshift_vec3_dot(a, b) == 32.0f);
    StickShiftVec3 z = stickshift_vec3_cross(x, y);
    REQUIRE(z.x == 0.0f && z.y == 0.0f && z.z == 1.0f);
    StickShiftVec3 three_four = {3, 4, 0};
    REQUIRE(stickshift_vec3_length(three_four) == 5.0f);
    StickShiftVec3 n = stickshift_vec3_normalize_or_zero(zero);
    REQUIRE(n.x == 0.0f && n.y == 0.0f && n.z == 0.0f);
    StickShiftVec3 back = {0, 0, -3};
    n = stickshift_vec3_normalize_or_zero(back);
    REQUIRE(n.x == 0.0f && n.y == 0.0f && n.z == -1.0f);
    StickShiftVec3 far = {2, 4, 8};
    StickShiftVec3 mid = stickshift_vec3_lerp(zero, far, 0.5f);
    REQUIRE(mid.x == 1.0f && mid.y == 2.0f && mid.z == 4.0f);
    /* lerp is a * (1 - t) + b * t, so t = 1 gives b exactly; a + (b - a) * t would give 0 here. */
    StickShiftVec3 huge = {1e8f, 1e8f, 1e8f}, unit = {1, 1, 1};
    StickShiftVec3 end = stickshift_vec3_lerp(huge, unit, 1.0f);
    REQUIRE(end.x == 1.0f && end.y == 1.0f && end.z == 1.0f);

    StickShiftQuat identity = stickshift_quat_from_rotation_y(0.0f);
    REQUIRE(identity.x == 0.0f && identity.y == 0.0f && identity.z == 0.0f && identity.w == 1.0f);
    identity = stickshift_quat_from_rotation_x(0.0f);
    REQUIRE(identity.w == 1.0f);
    identity = stickshift_quat_from_rotation_z(0.0f);
    REQUIRE(identity.w == 1.0f);
    /* The demo's building facing (a quarter turn about +Y) turns the travel axis -Z to +X. */
    StickShiftQuat facing = stickshift_quat_from_rotation_y(-1.57079632679f);
    REQUIRE(near3(stickshift_quat_mul_vec3(facing, ahead), x));
    StickShiftQuat same = stickshift_quat_mul(identity, facing);
    REQUIRE(same.x == facing.x && same.y == facing.y && same.z == facing.z && same.w == facing.w);
    /* Two quarter turns, one way and back, are no turn. */
    StickShiftQuat undo = stickshift_quat_from_rotation_y(1.57079632679f);
    REQUIRE(near3(stickshift_quat_mul_vec3(stickshift_quat_mul(undo, facing), ahead), ahead));

    StickShiftVec3 t = {1, 2, 3}, s = {2, 2, 2}, ones = {1, 1, 1};
    StickShiftQuat none = {0, 0, 0, 1};
    StickShiftMat4 m = stickshift_mat4_from_trs(t, none, s);
    StickShiftVec3 p = stickshift_mat4_transform_point3(m, ones);
    REQUIRE(p.x == 3.0f && p.y == 4.0f && p.z == 5.0f);
    StickShiftVec3 v = stickshift_mat4_transform_vector3(m, ones);
    REQUIRE(v.x == 2.0f && v.y == 2.0f && v.z == 2.0f);

    /* A transform's matrix is from_trs of its fields, bit for bit. */
    REQUIRE(sizeof(StickShiftTransform) == 10 * sizeof(float));
    StickShiftTransform turned = {t, facing, s};
    StickShiftMat4 from_transform = stickshift_transform_to_mat4(turned);
    StickShiftMat4 from_trs = stickshift_mat4_from_trs(t, facing, s);
    REQUIRE(memcmp(&from_transform, &from_trs, sizeof from_trs) == 0);
    /* Scale first, then rotation: -Z doubled, then turned to +X. */
    StickShiftVec3 two_ahead = {0, 0, -2}, two_x = {2, 0, 0};
    REQUIRE(near3(stickshift_mat4_transform_vector3(from_transform, ahead),
                  stickshift_quat_mul_vec3(facing, two_ahead)));
    REQUIRE(near3(stickshift_mat4_transform_vector3(from_transform, ahead), two_x));
    return 0;
}

int main(int argc, char **argv) {
    int use_null = argc > 1 && strcmp(argv[1], "--null") == 0;
    const char *model_path = argc > 1 + use_null ? argv[1 + use_null] : NULL;
    REQUIRE(model_path != NULL);
    REQUIRE(stickshift_api_version() == STICKSHIFT_API_VERSION);
    if (check_math() != 0) {
        return 1;
    }

    StickShiftContextDesc desc = {STICKSHIFT_API_VERSION, use_null ? STICKSHIFT_BACKEND_NULL : STICKSHIFT_BACKEND_METAL,
                             NULL, WIDTH, HEIGHT, 1.0f};
    StickShiftContext *ctx = NULL;
    StickShiftResult created = stickshift_context_create(&desc, &ctx);
    if (!use_null && created == STICKSHIFT_ERROR_NO_DEVICE) {
        /* CI sets STICKSHIFT_REQUIRE_GPU=1: a runner that lost its GPU must fail, not skip. */
        const char *require = getenv("STICKSHIFT_REQUIRE_GPU");
        if (require != NULL && (strcmp(require, "1") == 0 || strcmp(require, "true") == 0)) {
            fprintf(stderr, "c smoke: no Metal device, and STICKSHIFT_REQUIRE_GPU is set (%s)\n",
                    stickshift_last_error_message());
            return 1;
        }
        printf("c smoke: skipped (no Metal device: %s)\n", stickshift_last_error_message());
        return 0;
    }
    CHECK(created, STICKSHIFT_OK);

    StickShiftEvent events[16];
    uint32_t count = 0;
    CHECK(stickshift_poll_events(ctx, events, 16, &count), STICKSHIFT_OK);
    REQUIRE(count == 1 && events[0].kind == STICKSHIFT_EVENT_CONTEXT_READY);

    /* A red triangle, copied. */
    float triangle[3][9] = {
        {-0.6f, -0.5f, 0.0f, 0, 0, 1, 0.9f, 0.1f, 0.1f},
        {0.6f, -0.5f, 0.0f, 0, 0, 1, 0.9f, 0.1f, 0.1f},
        {0.0f, 0.6f, 0.0f, 0, 0, 1, 0.9f, 0.1f, 0.1f},
    };
    uint32_t indices[3] = {0, 1, 2};
    StickShiftMeshDesc mesh_desc = {STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_COLOR,
                               copied(triangle, sizeof triangle), copied(indices, sizeof indices)};
    StickShiftMesh mesh = {0};
    CHECK(stickshift_mesh_create(ctx, &mesh_desc, NULL, &mesh), STICKSHIFT_OK);
    REQUIRE(stickshift_resource_state(ctx, mesh.id) == STICKSHIFT_RESOURCE_STATE_READY);

    /* A 2x2 texture whose pixels are lent, not copied. */
    static const uint8_t white[16] = {255, 255, 255, 255, 255, 255, 255, 255,
                                      255, 255, 255, 255, 255, 255, 255, 255};
    StickShiftTextureDesc texture_desc = {2, 2, {white, sizeof white, count_release, NULL},
                                     STICKSHIFT_COLOR_SPACE_SRGB, 0};
    StickShiftTexture texture = {0};
    CHECK(stickshift_texture_create(ctx, &texture_desc, NULL, &texture), STICKSHIFT_OK);
    REQUIRE(releases == 1);

    /* A model, parsed off-thread; its event goes to the callback. */
    StickShiftCompletion completion = {on_model, NULL};
    StickShiftModel model = {0};
    CHECK(stickshift_model_load_file(ctx, model_path, &completion, &model), STICKSHIFT_OK);
    CHECK(stickshift_context_flush(ctx), STICKSHIFT_OK);
    REQUIRE(model_events == 1);
    CHECK(model_result, STICKSHIFT_OK);
    StickShiftVec3 lo, hi;
    CHECK(stickshift_model_get_bounds(ctx, model, &lo, &hi), STICKSHIFT_OK);
    REQUIRE(hi.x > lo.x);

    /* One frame: the triangle, the model (whole, then by part), two lights, a HUD bar, and a
     * read-back. */
    StickShiftFrame *frame = NULL;
    StickShiftFrame *second = NULL;
    CHECK(stickshift_frame_acquire(ctx, &frame), STICKSHIFT_OK);
    CHECK(stickshift_frame_acquire(ctx, &second), STICKSHIFT_ERROR_WRONG_STATE);
    REQUIRE(second == NULL);

    StickShiftVec3 eye = {0.0f, 0.5f, 3.0f}, target = {0.0f, 0.0f, 0.0f}, up = {0.0f, 1.0f, 0.0f};
    StickShiftCamera camera;
    camera.view_projection =
        stickshift_mat4_mul(stickshift_mat4_perspective(0.9f, (float)WIDTH / HEIGHT, 0.1f, 50.0f),
                       stickshift_mat4_look_at(eye, target, up));
    camera.position = eye;
    CHECK(stickshift_frame_set_camera(frame, &camera), STICKSHIFT_OK);

    StickShiftLight lamp = {STICKSHIFT_LIGHT_POINT, {0.5f, 1.0f, 1.0f}, {0.0f, 0.0f, 0.0f},
                            {1.0f, 0.8f, 0.6f}, 2.0f, 4.0f, 0.0f, 0.0f};
    StickShiftLight torch = {STICKSHIFT_LIGHT_SPOT, {0.0f, 0.5f, 2.0f}, {0.0f, 0.0f, -1.0f},
                             {0.6f, 0.8f, 1.0f}, 3.0f, 6.0f, 10.0f, 20.0f};
    CHECK(stickshift_frame_add_light(frame, &lamp), STICKSHIFT_OK);
    CHECK(stickshift_frame_add_light(frame, &torch), STICKSHIFT_OK);
    StickShiftLight no_range = lamp;
    no_range.range = 0.0f;
    CHECK(stickshift_frame_add_light(frame, &no_range), STICKSHIFT_ERROR_INVALID_ARGUMENT);

    StickShiftMat4 identity = stickshift_mat4_identity();
    StickShiftMaterial glowing = {{1, 1, 1, 1}, {0}, {0.4f, 0.3f, 0.1f}};
    CHECK(stickshift_frame_draw_mesh(frame, mesh, &identity, &glowing), STICKSHIFT_OK);
    StickShiftVec3 beside = {1.5f, 0.0f, -1.0f}, scale = {0.5f, 0.5f, 0.5f};
    StickShiftQuat spin = stickshift_quat_from_axis_angle(up, 0.6f);
    StickShiftMat4 world = stickshift_mat4_from_trs(beside, spin, scale);
    CHECK(stickshift_frame_draw_model(frame, model, &world, NULL), STICKSHIFT_OK);
    /* The model again, at world x (local x node) with an identity local matrix: every part,
     * then its textured parts, then its untextured ones, which add up to every part. */
    StickShiftModelDraw by_part = {world, identity, {1, 1, 1, 1}, STICKSHIFT_MODEL_PARTS_ALL};
    CHECK(stickshift_frame_draw_model_ex(frame, model, &by_part), STICKSHIFT_OK);
    by_part.parts = STICKSHIFT_MODEL_PARTS_TEXTURED;
    CHECK(stickshift_frame_draw_model_ex(frame, model, &by_part), STICKSHIFT_OK);
    by_part.parts = STICKSHIFT_MODEL_PARTS_UNTEXTURED;
    CHECK(stickshift_frame_draw_model_ex(frame, model, &by_part), STICKSHIFT_OK);
    by_part.parts = 7;
    CHECK(stickshift_frame_draw_model_ex(frame, model, &by_part), STICKSHIFT_ERROR_INVALID_ARGUMENT);
    StickShiftQuad bar = {2, 2, 20, 4, 0, 0, 1, 1, {1, 1, 1, 0.8f}, STICKSHIFT_FILL_SOLID, {0}};
    CHECK(stickshift_frame_draw_quad(frame, &bar), STICKSHIFT_OK);

    static uint8_t pixels[WIDTH * HEIGHT * 4];
    memset(pixels, 0xAB, sizeof pixels);
    CHECK(stickshift_frame_read_pixels_async(frame, pixels, sizeof pixels, NULL), STICKSHIFT_OK);
    uint64_t number = 0;
    CHECK(stickshift_frame_submit(frame, NULL, &number), STICKSHIFT_OK);
    REQUIRE(number == 1);

    CHECK(stickshift_context_flush(ctx), STICKSHIFT_OK);
    int completed = 0, read_back = 0;
    do {
        CHECK(stickshift_poll_events(ctx, events, 16, &count), STICKSHIFT_OK);
        for (uint32_t i = 0; i < count; i++) {
            completed |= events[i].kind == STICKSHIFT_EVENT_FRAME_COMPLETED && events[i].frame == 1;
            read_back |= events[i].kind == STICKSHIFT_EVENT_READBACK_COMPLETE && events[i].frame == 1;
        }
    } while (count == 16);
    REQUIRE(completed && read_back);

    /* Frame 1's GPU time is known now that it has completed: zero on the null backend, and
     * under a second on Metal (a virtualized GPU's coarse timestamps can also read zero). */
    uint64_t gpu_ns = UINT64_MAX;
    CHECK(stickshift_context_gpu_frame_time(ctx, 1, &gpu_ns), STICKSHIFT_OK);
    REQUIRE(use_null ? gpu_ns == 0 : gpu_ns < 1000000000u);
    CHECK(stickshift_context_gpu_frame_time(ctx, 2, &gpu_ns), STICKSHIFT_ERROR_INVALID_ARGUMENT);

    size_t center = ((HEIGHT / 2) * WIDTH + WIDTH / 2) * 4, corner = (HEIGHT - 1) * WIDTH * 4;
    if (use_null) {
        for (size_t i = 0; i < sizeof pixels; i++) REQUIRE(pixels[i] == 0);
    } else {
        /* The triangle covers the centre; the bottom-left corner is sky. */
        REQUIRE(memcmp(pixels + center, pixels + corner, 4) != 0);
        REQUIRE(pixels[center + 2] > pixels[center + 1]); /* BGRA: red dominates */
    }

    StickShiftStats stats;
    CHECK(stickshift_context_get_stats(ctx, &stats), STICKSHIFT_OK);
    REQUIRE(stats.frames_submitted == 1 && stats.frames_completed == 1);
    /* The triangle, then each of the model's parts three times: whole, all by part, and split. */
    REQUIRE(stats.draws_submitted >= 4 && (stats.draws_submitted - 1) % 3 == 0);
    REQUIRE(stats.draws_skipped == 0);
    REQUIRE(stats.meshes == 1 && stats.textures == 1 && stats.models == 1);

    CHECK(stickshift_model_destroy(ctx, model), STICKSHIFT_OK);
    CHECK(stickshift_model_destroy(ctx, model), STICKSHIFT_ERROR_INVALID_HANDLE);
    REQUIRE(strlen(stickshift_last_error_message()) > 0);
    CHECK(stickshift_context_destroy(ctx), STICKSHIFT_OK);

    printf("c smoke: OK (%s backend, frame %llu)\n", use_null ? "null" : "Metal",
           (unsigned long long)number);
    return 0;
}
