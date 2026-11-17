/*
 * The engine services smoke test: a C game reading input, following with the chase camera
 * and counting sounds, and a C host building its Graphics menu from the engine's data.
 *
 *   services
 *
 * Creates a null context and a silent engine, pushes key events and checks that the game's
 * updates see each edge exactly once, follows a target with the chase camera, plays sounds and
 * reads the audio counters, walks the Graphics menu by its strides and chooses an item from
 * it, and checks that a host-only call is refused inside a hook. Exits 0 on success, 1 on
 * failure.
 *
 * Run by scripts/test-c-api.sh (and CI).
 *
 * Copyright (c) 2026 Parsa Bagheri. Released under the Apache-2.0 License.
 */

#include <stdio.h>
#include <string.h>

#include "stickshift.h"

#define CHECK(expr, expected)                                                              \
    do {                                                                                   \
        StickShiftResult check_result_ = (expr);                                           \
        if (check_result_ != (expected)) {                                                 \
            fprintf(stderr, "%s:%d: %s returned %s, expected %s (%s)\n", __FILE__,         \
                    __LINE__, #expr, stickshift_result_name(check_result_), #expected,     \
                    stickshift_last_error_message());                                      \
            return 1;                                                                      \
        }                                                                                  \
    } while (0)

#define REQUIRE(cond)                                                                      \
    do {                                                                                   \
        if (!(cond)) {                                                                     \
            fprintf(stderr, "%s:%d: requirement failed: %s\n", __FILE__, __LINE__, #cond); \
            return 1;                                                                      \
        }                                                                                  \
    } while (0)

/* What the hooks saw. Hooks cannot fail the test themselves, so they record. */
typedef struct Game {
    int updates, space_presses, space_releases, failures;
    StickShiftSound chime;
    StickShiftResult state_in_hook; /* stickshift_graphics_get_state from update */
    float camera_z;                 /* the chase camera's eye, from render */
} Game;

static void expect_ok(Game *game, StickShiftResult r) {
    if (r != STICKSHIFT_OK) game->failures++;
}

static StickShiftResult init(void *user, StickShiftEngine *engine) {
    Game *game = user;
    StickShiftResult r = stickshift_camera_set_clip_planes(engine, 0.5f, 500.0f);
    if (r != STICKSHIFT_OK) return r;
    return stickshift_audio_load(engine, "sounds/chime.wav", &game->chime);
}

static void update(void *user, StickShiftEngine *engine, float dt) {
    Game *game = user;
    game->updates++;
    StickShiftInputState input;
    memset(&input, 0, sizeof input);
    input.size = sizeof input;
    expect_ok(game, stickshift_input_get(engine, &input));
    if (input.keys[STICKSHIFT_KEY_SPACE] & STICKSHIFT_KEY_STATE_JUST_PRESSED) {
        game->space_presses++;
        expect_ok(game, stickshift_audio_play_once(engine, game->chime, -6.0f));
    }
    if (input.keys[STICKSHIFT_KEY_SPACE] & STICKSHIFT_KEY_STATE_JUST_RELEASED) game->space_releases++;

    StickShiftChase chase;
    stickshift_chase_init(&chase, 8.0f, 4.0f);
    StickShiftVec3 target = {0.0f, 0.0f, -(float)game->updates};
    StickShiftVec3 forward = {0.0f, 0.0f, -1.0f};
    expect_ok(game, stickshift_camera_follow(engine, &chase, target, forward, dt));

    StickShiftGraphicsState state;
    memset(&state, 0, sizeof state);
    state.size = sizeof state;
    game->state_in_hook = stickshift_graphics_get_state(engine, &state);
}

static void render(void *user, StickShiftEngine *engine, StickShiftFrame *frame) {
    Game *game = user;
    (void)frame;
    StickShiftCameraState camera;
    memset(&camera, 0, sizeof camera);
    camera.size = sizeof camera;
    expect_ok(game, stickshift_camera_get(engine, &camera));
    game->camera_z = camera.position.z;
}

static StickShiftResult push(StickShiftEngine *engine, const StickShiftHostEvent *events,
                             uint32_t count, uint32_t *requests) {
    return stickshift_engine_push_events(engine, events, count, sizeof events[0], requests);
}

int main(void) {
    StickShiftContextDesc ctx_desc = {STICKSHIFT_API_VERSION, STICKSHIFT_BACKEND_NULL, NULL,
                                      1280, 720, 1.0f};
    StickShiftContext *ctx = NULL;
    CHECK(stickshift_context_create(&ctx_desc, &ctx), STICKSHIFT_OK);

    Game game;
    memset(&game, 0, sizeof game);
    StickShiftEngineDesc desc;
    memset(&desc, 0, sizeof desc);
    desc.api_version = STICKSHIFT_API_VERSION;
    desc.size = sizeof desc;
    StickShiftGame hooks = {sizeof hooks, 0, &game, init, update, render, NULL, NULL};
    StickShiftEngine *engine = NULL;
    CHECK(stickshift_engine_create(ctx, &desc, &hooks, &engine), STICKSHIFT_OK);
    CHECK(stickshift_engine_init(engine), STICKSHIFT_OK);
    REQUIRE(game.chime.id != 0);

    StickShiftTickDesc one = {sizeof one, STICKSHIFT_CLOCK_FIXED_STEP, 0, NULL, 0};
    StickShiftTickDesc none = {sizeof none, STICKSHIFT_CLOCK_ELAPSED, 0, NULL, 0};
    StickShiftTickResult result;
    memset(&result, 0, sizeof result);
    result.size = sizeof result;

    /* A tap between two frames: both edges, in the next update only, even across a frame
       that runs no update. */
    StickShiftHostEvent events[3];
    memset(events, 0, sizeof events);
    events[0].kind = STICKSHIFT_HOST_KEY_DOWN;
    events[0].code = STICKSHIFT_KEY_SPACE;
    events[1].kind = STICKSHIFT_HOST_KEY_UP;
    events[1].code = STICKSHIFT_KEY_SPACE;
    uint32_t requests = 0;
    CHECK(push(engine, events, 2, &requests), STICKSHIFT_OK);
    CHECK(stickshift_engine_tick(engine, &none, &result), STICKSHIFT_OK);
    REQUIRE(result.steps == 0 && game.space_presses == 0);
    CHECK(stickshift_engine_tick(engine, &one, &result), STICKSHIFT_OK);
    CHECK(stickshift_engine_tick(engine, &one, &result), STICKSHIFT_OK);
    REQUIRE(game.updates == 2 && game.space_presses == 1 && game.space_releases == 1);

    /* A held key, its repeat (no new edge), then focus loss (a release edge). */
    memset(events, 0, sizeof events);
    events[0].kind = STICKSHIFT_HOST_KEY_DOWN;
    events[0].code = STICKSHIFT_KEY_SPACE;
    events[1].kind = STICKSHIFT_HOST_KEY_DOWN;
    events[1].code = STICKSHIFT_KEY_SPACE;
    events[1].flags = STICKSHIFT_HOST_EVENT_REPEAT;
    events[2].kind = STICKSHIFT_HOST_CURSOR;
    events[2].x = 10.5;
    events[2].y = 20.25;
    CHECK(push(engine, events, 3, &requests), STICKSHIFT_OK);
    CHECK(stickshift_engine_tick(engine, &one, &result), STICKSHIFT_OK);
    memset(events, 0, sizeof events);
    events[0].kind = STICKSHIFT_HOST_KEY_DOWN;
    events[0].code = STICKSHIFT_KEY_SPACE;
    events[0].flags = STICKSHIFT_HOST_EVENT_REPEAT;
    CHECK(push(engine, events, 1, &requests), STICKSHIFT_OK);
    CHECK(stickshift_engine_tick(engine, &one, &result), STICKSHIFT_OK);
    REQUIRE(game.space_presses == 2 && game.space_releases == 1);
    StickShiftInputState input;
    memset(&input, 0, sizeof input);
    input.size = sizeof input;
    CHECK(stickshift_input_get(engine, &input), STICKSHIFT_OK);
    REQUIRE(input.keys[STICKSHIFT_KEY_SPACE] == STICKSHIFT_KEY_STATE_DOWN);
    REQUIRE(input.has_cursor == 1 && input.cursor_x == 10.5 && input.cursor_y == 20.25);
    events[0].kind = STICKSHIFT_HOST_FOCUS_LOST;
    events[0].code = 0;
    events[0].flags = 0;
    CHECK(push(engine, events, 1, &requests), STICKSHIFT_OK);
    CHECK(stickshift_engine_tick(engine, &one, &result), STICKSHIFT_OK);
    REQUIRE(game.space_releases == 2);

    /* The chase camera trails 8 behind the target the last update set. */
    REQUIRE(game.failures == 0);
    REQUIRE(game.camera_z == 8.0f - (float)game.updates);
    CHECK(game.state_in_hook, STICKSHIFT_ERROR_WRONG_STATE);

    /* Audio: one sound, two one-shots asked for, nothing decoded in silent mode. */
    StickShiftAudioStats stats;
    memset(&stats, 0, sizeof stats);
    stats.size = sizeof stats;
    CHECK(stickshift_audio_get_stats(engine, &stats), STICKSHIFT_OK);
    REQUIRE(stats.sound_count == 1 && stats.decode_count == 0 && stats.is_silent == 1);
    REQUIRE(stats.one_shots_played == 2 && stats.loop_starts == 0 && stats.looping.id == 0);
    const char *path = NULL;
    CHECK(stickshift_audio_get_path(engine, game.chime, &path), STICKSHIFT_OK);
    REQUIRE(strcmp(path, "sounds/chime.wav") == 0);
    StickShiftSound stale = {game.chime.id + 1};
    CHECK(stickshift_audio_play_once(engine, stale, 0.0f), STICKSHIFT_ERROR_INVALID_HANDLE);

    /* The Graphics menu as data: find Draw Distance > Near by walking the strides. */
    StickShiftMenu menu;
    memset(&menu, 0, sizeof menu);
    menu.size = sizeof menu;
    CHECK(stickshift_engine_get_menu(engine, 0, &menu), STICKSHIFT_OK);
    REQUIRE(strcmp(menu.title, "Graphics") == 0 && menu.group_count == 5 && menu.revision == 0);
    int32_t near_tag = 0;
    int checked = 0;
    for (uint32_t g = 0; g < menu.group_count; g++) {
        const StickShiftMenuGroup *group =
            (const StickShiftMenuGroup *)((const char *)menu.groups + g * menu.group_stride);
        for (uint32_t i = 0; i < group->item_count; i++) {
            const StickShiftMenuItem *item =
                (const StickShiftMenuItem *)((const char *)group->items + i * group->item_stride);
            checked += item->checked;
            if (group->title && strcmp(group->title, "Draw Distance") == 0 &&
                strcmp(item->label, stickshift_graphics_label(STICKSHIFT_GRAPHICS_SECTION_DRAW_DISTANCE,
                                                              STICKSHIFT_DRAW_DISTANCE_NEAR)) == 0) {
                near_tag = item->tag;
            }
        }
    }
    REQUIRE(near_tag == 301 && checked == 4);
    memset(events, 0, sizeof events);
    events[0].kind = STICKSHIFT_HOST_MENU;
    events[0].code = (uint32_t)near_tag;
    CHECK(push(engine, events, 1, &requests), STICKSHIFT_OK);
    REQUIRE(requests == STICKSHIFT_REQUEST_MENU_SYNC);
    StickShiftGraphicsSettings current;
    CHECK(stickshift_graphics_get_current(engine, &current), STICKSHIFT_OK);
    REQUIRE(current.draw_distance == STICKSHIFT_DRAW_DISTANCE_NEAR);
    StickShiftGraphicsState state;
    memset(&state, 0, sizeof state);
    state.size = sizeof state;
    CHECK(stickshift_graphics_get_state(engine, &state), STICKSHIFT_OK);
    REQUIRE(state.revision == 1 && state.has_pending == 0 && state.base_spawn_ahead == 60.0f);
    CHECK(stickshift_engine_get_menu(engine, 1, &menu), STICKSHIFT_OK);
    REQUIRE(menu.revision == 1);

    /* The settings as the four codes they persist as. */
    int64_t codes[4];
    CHECK(stickshift_graphics_settings_to_codes(&current, codes), STICKSHIFT_OK);
    REQUIRE(codes[0] == 3 && codes[1] == 1 && codes[2] == 1 && codes[3] == 1);
    StickShiftGraphicsSettings decoded;
    stickshift_graphics_settings_from_codes(codes, &decoded);
    REQUIRE(memcmp(&decoded, &current, sizeof decoded) == 0);

    CHECK(stickshift_engine_destroy(engine), STICKSHIFT_OK);
    CHECK(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    printf("c services: OK (%d updates, %u one-shots)\n", game.updates, stats.one_shots_played);
    return 0;
}
