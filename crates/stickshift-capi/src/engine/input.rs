// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

//! The input's read side through C. The host writes it with
//! `stickshift_engine_push_events`; the game reads it here.
//!
//! The engine's input cannot change inside a tick (events arrive between
//! engine calls), so one snapshot per hook is exact. Edges clear only after
//! each update step, never per tick or per event batch.

use super::{button, game_call, key, Access, StickShiftEngine};
use crate::core::write_sized;
use crate::types::*;

/// The keyboard, mouse buttons and cursor as the game sees them: in a hook,
/// what this hook's `EngineCtx` input shows; between engine calls, the input
/// as the events so far left it. Set `out->size = sizeof(StickShiftInputState)`.
///
/// # Safety
/// `engine` must be NULL or a live engine; `out` NULL or valid for its
/// declared size.
#[no_mangle]
pub unsafe extern "C" fn stickshift_input_get(
    engine: *mut StickShiftEngine,
    out: *mut StickShiftInputState,
) -> StickShiftResult {
    game_call(engine, "stickshift_input_get", |_, access| {
        let input = match &access {
            Access::Idle(state) => &state.session.lp.input,
            Access::Hook(ctx) => ctx.input(),
        };
        let mut state = StickShiftInputState::default();
        for (code, bits) in (0..).zip(state.keys.iter_mut()) {
            if let Some(key) = key(code) {
                *bits = [
                    (input.is_key_down(key), STICKSHIFT_KEY_STATE_DOWN),
                    (
                        input.is_key_just_pressed(key),
                        STICKSHIFT_KEY_STATE_JUST_PRESSED,
                    ),
                    (
                        input.is_key_just_released(key),
                        STICKSHIFT_KEY_STATE_JUST_RELEASED,
                    ),
                ]
                .into_iter()
                .filter(|(on, _)| *on)
                .fold(0, |bits, (_, bit)| bits | bit);
            }
        }
        for (code, held) in (0..).zip(state.mouse_buttons.iter_mut()) {
            if let Some(button) = button(code) {
                *held = u8::from(input.is_mouse_down(button));
            }
        }
        if let Some((x, y)) = input.cursor_position() {
            state.has_cursor = 1;
            state.cursor_x = x;
            state.cursor_y = y;
        }
        write_sized(out, state, "out")?;
        Ok(STICKSHIFT_OK)
    })
}
