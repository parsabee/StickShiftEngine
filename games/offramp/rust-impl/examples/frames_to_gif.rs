// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! Assembles PNG frames into a looping animated GIF, scaling each frame to a
//! given width. Used by `scripts/record-demo-gif.sh`.
//!
//! ```sh
//! cargo run --release -p offramp --example frames_to_gif -- \
//!     <out.gif> <width> <seconds-per-frame> <frame.png>...
//! ```

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;

use image::codecs::gif::{GifEncoder, Repeat};
use image::imageops::FilterType;
use image::{Delay, Frame};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!("usage: frames_to_gif <out.gif> <width> <seconds-per-frame> <frame.png>...");
        ExitCode::from(2)
    };
    if args.len() < 4 {
        return usage();
    }
    let (Ok(width), Ok(delay)) = (args[1].parse::<u32>(), args[2].parse::<f64>()) else {
        return usage();
    };
    if width == 0 || delay.is_nan() || delay <= 0.0 {
        return usage();
    }

    let out = &args[0];
    let file = File::create(out).unwrap_or_else(|e| panic!("cannot create {out}: {e}"));
    let mut encoder = GifEncoder::new_with_speed(BufWriter::new(file), 10);
    // Loop forever.
    encoder
        .set_repeat(Repeat::Infinite)
        .expect("set the GIF's loop count");
    let delay = Delay::from_numer_denom_ms((delay * 1000.0).round() as u32, 1);

    for path in &args[3..] {
        let frame = image::open(path)
            .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
            .to_rgba8();
        let height = u64::from(frame.height()) * u64::from(width) / u64::from(frame.width());
        let scaled =
            image::imageops::resize(&frame, width, height.max(1) as u32, FilterType::Lanczos3);
        encoder
            .encode_frame(Frame::from_parts(scaled, 0, 0, delay))
            .unwrap_or_else(|e| panic!("cannot write {out}: {e}"));
    }
    ExitCode::SUCCESS
}
