# Benchmarks

How fast the renderer is on real hardware, measured with the benchmark example in
[`crates/stickshift-render/examples/bench.rs`](../crates/stickshift-render/examples/bench.rs):

```sh
cargo run --release -p stickshift-render --example bench
```

It prints the device and a Markdown table like the ones below. Run it in release mode; a debug
build says so, because its CPU times are meaningless.

## What it measures

Each scenario draws a square grid of meshes under the default sun. Half the meshes use the
untextured pipeline and half the textured one (a 256 × 256 sRGB checkerboard), grouped by
pipeline as a game would draw them. They render into a **1920 × 1080 offscreen target with 4×
MSAA**, with shadows at High unless the scenario says otherwise. The light scenarios also add
point lights on a grid over the cubes every frame, each reaching a couple of cubes in every
direction. After 60 warm-up frames, it times 300 frames:

- **CPU time** runs from `begin_frame` through `submit_async`: recording the draws and lights,
  binning the lights into clusters, and encoding the shadow and scene passes on the calling thread.
- **GPU time** is the frame's command-buffer execution time, start to end on the GPU, as
  `MetalRenderer::gpu_frame_time` reports it.
- **Frames/s** is frames over wall time with up to three frames in flight and no vsync. That makes
  it a throughput figure, not a display rate. It can exceed `1 / GPU time`, because consecutive
  frames overlap on the GPU.

Means and 95th percentiles are over the 300 measured frames.

## Results

### Apple M3 Max (40-core GPU, 48 GB), macOS 26.6.2

`v0.2` development build (point and spot lights), November 2026:

| Scenario | Draws | Triangles | CPU ms (mean / p95) | GPU ms (mean / p95) | Frames/s |
|---|---:|---:|---:|---:|---:|
| 100 cubes | 100 | 1,200 | 0.12 / 0.23 | 0.14 / 0.24 | 8147 |
| 1,000 cubes | 1,000 | 12,000 | 0.28 / 0.28 | 0.16 / 0.16 | 3610 |
| 1,000 cubes, shadows off | 1,000 | 12,000 | 0.20 / 0.24 | 0.08 / 0.08 | 4852 |
| 1,000 cubes, 64 lights | 1,000 | 12,000 | 0.28 / 0.29 | 0.19 / 0.19 | 3526 |
| 1,000 cubes, 256 lights | 1,000 | 12,000 | 0.29 / 0.31 | 0.27 / 0.28 | 3382 |
| 5,000 cubes | 5,000 | 60,000 | 1.17 / 1.24 | 0.47 / 0.48 | 853 |
| 16 dense spheres (1M triangles) | 16 | 1,048,576 | 0.74 / 0.86 | 0.92 / 0.96 | 1337 |

The scenarios without lights match the `v0.1.0` build's figures within run-to-run noise.

Repeated runs agree within a few percent.

## What the numbers say

- **Draw calls cost CPU time, about 0.24 µs each.** CPU time grows with the number of draws, from
  0.12 ms for 100 to 1.17 ms for 5,000, because every draw writes its own uniforms and is encoded
  twice (shadow pass and scene pass). That is cheap here, but it is the first cost to watch on
  slower CPUs; instancing repeated meshes would cut it.
- **The shadow pass is about half the GPU time** of a simple scene. Turning shadows off takes
  1,000 cubes from 0.16 ms to 0.08 ms of GPU time.
- **Lights are cheap where they are binned.** 256 point lights add 0.11 ms of GPU time to 1,000
  cubes at 1080p, and 0.01 to 0.02 ms of CPU time for binning them. Each fragment shades only the
  lights in its own cluster (see [ARCHITECTURE §4c](ARCHITECTURE.md#4c-point-and-spot-lights)),
  so the cost follows how much of the screen each light covers more than how many there are.
- **Triangle count is cheap on this GPU.** A million triangles across 16 draws takes 0.92 ms of GPU
  time and 0.74 ms of CPU time (mostly encoding two passes over large index buffers).
- **Nothing here is near a frame budget.** The heaviest scenario uses well under 10% of a 120 Hz
  frame on either processor. The interesting numbers will come from iPhone hardware.

## Adding results

Run the example on another machine and add a section for it: the device (the first line of the
output), the OS version, the engine version or commit, and the table. iPhone results will come
with iOS support in v0.9.
