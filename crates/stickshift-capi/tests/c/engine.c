/*
 * The engine smoke test: a C game, built against stickshift.h and libstickshift.
 *
 *   engine
 *
 * Creates a null context and an engine with a game of plain C callbacks, runs its
 * init, ticks it, and reads what the frames recorded with stickshift_context_get_last_frame.
 * Along the way: host events (a key, a menu choice, close), the scope rules (an engine
 * call a hook may not make), the engine's state, and destroy's release. Exits 0 on
 * success, 1 on failure.
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
    int inits, updates, renders, settings_changes, releases;
    StickShiftResult tick_from_hook;  /* stickshift_engine_tick called from update */
    StickShiftResult draw_result;     /* the render hook's quad */
    StickShiftResult acquire_in_render;
    float dt;
} Game;

static StickShiftResult init(void *user, StickShiftEngine *engine) {
    Game *game = user;
    game->inits++;
    /* The sun is sticky on the context, so it lights the first frame. */
    StickShiftContext *ctx = NULL;
    StickShiftResult r = stickshift_engine_get_context(engine, &ctx);
    if (r != STICKSHIFT_OK) return r;
    StickShiftSunSky sun;
    stickshift_sun_sky_default(&sun);
    sun.sun_elevation_deg = 30.0f;
    return stickshift_context_set_sun_sky(ctx, &sun);
}

static void update(void *user, StickShiftEngine *engine, float dt) {
    Game *game = user;
    game->updates++;
    game->dt = dt;
    StickShiftTickDesc tick = {sizeof tick, STICKSHIFT_CLOCK_FIXED_STEP, 0, NULL, 0};
    game->tick_from_hook = stickshift_engine_tick(engine, &tick, NULL);
}

static void render(void *user, StickShiftEngine *engine, StickShiftFrame *frame) {
    Game *game = user;
    game->renders++;
    StickShiftQuad q = {0, 0, 10, 10, 0, 0, 1, 1, {1, 1, 1, 1}, STICKSHIFT_FILL_SOLID, {0}};
    game->draw_result = stickshift_frame_draw_quad(frame, &q);
    StickShiftContext *ctx = NULL;
    StickShiftFrame *second = NULL;
    stickshift_engine_get_context(engine, &ctx);
    game->acquire_in_render = stickshift_frame_acquire(ctx, &second);
}

static void settings_changed(void *user, StickShiftEngine *engine,
                             const StickShiftGraphicsSettings *settings) {
    Game *game = user;
    (void)engine;
    (void)settings;
    game->settings_changes++;
}

static void release(void *user) {
    Game *game = user;
    game->releases++;
}

int main(void) {
    REQUIRE(stickshift_api_version() == STICKSHIFT_API_VERSION);
    REQUIRE(stickshift_api_minor() >= STICKSHIFT_API_MINOR);
    REQUIRE(stickshift_fixed_dt() > 0.0166f && stickshift_fixed_dt() < 0.0167f);

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
    desc.flags = STICKSHIFT_ENGINE_QUIT_ON_ESCAPE;
    StickShiftGame hooks = {sizeof hooks, 0, &game, init, update, render, settings_changed, release};
    StickShiftEngine *engine = NULL;

    /* A forgotten size fails loudly. */
    StickShiftEngineDesc unsized = desc;
    unsized.size = 0;
    CHECK(stickshift_engine_create(ctx, &unsized, &hooks, &engine), STICKSHIFT_ERROR_INVALID_ARGUMENT);
    REQUIRE(engine == NULL);

    CHECK(stickshift_engine_create(ctx, &desc, &hooks, &engine), STICKSHIFT_OK);
    REQUIRE(game.inits == 0);
    StickShiftTickDesc tick = {sizeof tick, STICKSHIFT_CLOCK_FIXED_STEP, 0, NULL, 0};
    StickShiftTickResult result;
    memset(&result, 0, sizeof result);
    result.size = sizeof result;
    CHECK(stickshift_engine_tick(engine, &tick, &result), STICKSHIFT_ERROR_WRONG_STATE);

    CHECK(stickshift_engine_init(engine), STICKSHIFT_OK);
    REQUIRE(game.inits == 1);
    CHECK(stickshift_engine_init(engine), STICKSHIFT_OK);
    REQUIRE(game.inits == 1);

    for (int i = 0; i < 3; i++) {
        CHECK(stickshift_engine_tick(engine, &tick, &result), STICKSHIFT_OK);
        REQUIRE(result.steps == 1 && result.rendered == 1 && result.submitted == 1);
        REQUIRE(result.frame == (uint64_t)(i + 1));
    }
    REQUIRE(game.updates == 3 && game.renders == 3);
    REQUIRE(game.dt == stickshift_fixed_dt());
    CHECK(game.tick_from_hook, STICKSHIFT_ERROR_WRONG_STATE);
    CHECK(game.draw_result, STICKSHIFT_OK);
    CHECK(game.acquire_in_render, STICKSHIFT_ERROR_WRONG_STATE);

    /* What the last frame recorded: the one quad, under the sun init set. */
    StickShiftFrameRecord record;
    memset(&record, 0, sizeof record);
    record.size = sizeof record;
    CHECK(stickshift_context_get_last_frame(ctx, &record), STICKSHIFT_OK);
    REQUIRE(record.submitted == 1 && record.frame_number == 3);
    REQUIRE(record.quad_count == 1 && record.draw_count == 0);
    REQUIRE(record.quad_stride == sizeof(StickShiftQuad));
    REQUIRE(record.quads[0].width == 10.0f && record.quads[0].fill == STICKSHIFT_FILL_SOLID);
    REQUIRE(record.sun.sun_elevation_deg == 30.0f);

    /* Host events: a menu choice applies at once and changes the revision. */
    StickShiftHostEvent events[2];
    memset(events, 0, sizeof events);
    events[0].kind = STICKSHIFT_HOST_MENU;
    events[0].code = 101; /* Shadows: Off */
    uint32_t requests = 0;
    CHECK(stickshift_engine_push_events(engine, events, 1, sizeof events[0], &requests), STICKSHIFT_OK);
    REQUIRE(requests == STICKSHIFT_REQUEST_MENU_SYNC);
    REQUIRE(game.settings_changes == 1);
    CHECK(stickshift_engine_tick(engine, &tick, &result), STICKSHIFT_OK);
    REQUIRE(result.graphics_revision == 1);

    /* Escape with QUIT_ON_ESCAPE asks the host to exit; an invalid event applies nothing. */
    events[0].kind = STICKSHIFT_HOST_KEY_DOWN;
    events[0].code = STICKSHIFT_KEY_ESCAPE;
    events[1].kind = 99;
    CHECK(stickshift_engine_push_events(engine, events, 2, sizeof events[0], &requests),
          STICKSHIFT_ERROR_INVALID_ARGUMENT);
    CHECK(stickshift_engine_push_events(engine, events, 1, sizeof events[0], &requests), STICKSHIFT_OK);
    REQUIRE(requests == STICKSHIFT_REQUEST_EXIT);

    /* The game asks to exit: every later tick says so. */
    CHECK(stickshift_engine_request_exit(engine), STICKSHIFT_OK);
    CHECK(stickshift_engine_tick(engine, &tick, &result), STICKSHIFT_OK);
    REQUIRE(result.requests & STICKSHIFT_REQUEST_EXIT);

    StickShiftEngineState state;
    memset(&state, 0, sizeof state);
    state.size = sizeof state;
    CHECK(stickshift_engine_get_state(engine, &state), STICKSHIFT_OK);
    REQUIRE(state.initialized == 1 && state.hosted == 0 && state.failed == 0);
    REQUIRE(state.exit_requested == 1 && state.frames == 5 && state.frames_rendered == 5);
    REQUIRE(state.updates == 5 && state.frames_discarded == 0);
    StickShiftPerf perf;
    memset(&perf, 0, sizeof perf);
    perf.size = sizeof perf;
    CHECK(stickshift_engine_get_perf(engine, &perf), STICKSHIFT_OK);
    REQUIRE(perf.frame_count == 5 && perf.alpha == 0.0f);

    /* The context outlives its engine: destroy the engine first. */
    CHECK(stickshift_context_destroy(ctx), STICKSHIFT_ERROR_WRONG_STATE);
    CHECK(stickshift_engine_destroy(engine), STICKSHIFT_OK);
    REQUIRE(game.releases == 1);
    CHECK(stickshift_context_destroy(ctx), STICKSHIFT_OK);

    printf("c engine: OK (%d updates, %d renders)\n", game.updates, game.renders);
    return 0;
}
