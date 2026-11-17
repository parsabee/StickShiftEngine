/*
 * The engine smoke test: a C game, built against stickshift.h and libstickshift.
 *
 *   engine
 *
 * Creates a null context and an engine with a game of plain C callbacks, runs its
 * init, ticks it, and reads what the frames recorded with stickshift_context_get_last_frame.
 * Along the way: host events (a key, a menu choice, close), the scope rules (an engine
 * call a hook may not make), the engine's state, and destroy's release. A second game
 * registers a component, streams slots with a spawn callback that spawns whole bundles and
 * tracks them, and the host queries and despawns. Exits 0 on success, 1 on failure.
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

/* The streaming game: a registered tag, a stream pass with a C spawn callback, queries
 * and a despawn. */
typedef struct Streamer {
    StickShiftComponent slot_tag;
    int spawned, tracked;
    StickShiftResult config_in_spawn; /* scene calls are refused in a spawn callback */
    StickShiftResult stream_in_spawn;
    StickShiftResult track_in_update; /* tracking needs a spawn callback */
    StickShiftResult spawn_in_render; /* render only reads the world */
    uint32_t count_in_render;
} Streamer;

static void spawn_slot(void *user, StickShiftEngine *e, int64_t slot, StickShiftVec3 p) {
    Streamer *s = user;
    (void)slot;
    StickShiftTransform t = {p, {0, 0, 0, 1}, {1, 1, 1}};
    StickShiftRenderComponent r = {{0.5f, 0.5f, 0.5f}, STICKSHIFT_RENDER_SHAPE_CUBE};
    StickShiftComponentValue bundle[] = {
        {{STICKSHIFT_COMPONENT_TRANSFORM}, &t, sizeof t},
        {{STICKSHIFT_COMPONENT_RENDER}, &r, sizeof r},
        {s->slot_tag, NULL, 0},
        {{STICKSHIFT_COMPONENT_STATIC_TAG}, NULL, 0},
    };
    StickShiftEntity entity;
    if (stickshift_world_spawn(e, bundle, 4, &entity) == STICKSHIFT_OK) {
        s->spawned++;
        if (stickshift_stream_track(e, entity) == STICKSHIFT_OK) s->tracked++;
    }
    StickShiftStreamingConfig config;
    s->config_in_spawn = stickshift_scene_get_config(e, &config);
    s->stream_in_spawn = stickshift_scene_stream(e, p, NULL, NULL);
}

static StickShiftResult stream_init(void *user, StickShiftEngine *e) {
    Streamer *s = user;
    StickShiftComponentDesc d = {"smoke.SlotTag", 0, 1};
    StickShiftResult r = stickshift_component_register(e, &d, &s->slot_tag);
    if (r < 0) return r;
    StickShiftVec3 origin = {0, 0, 0};
    return stickshift_scene_stream(e, origin, spawn_slot, s);
}

static void stream_update(void *user, StickShiftEngine *e, float dt) {
    Streamer *s = user;
    (void)dt;
    StickShiftEntity none = {0};
    s->track_in_update = stickshift_stream_track(e, none);
}

static void stream_render(void *user, StickShiftEngine *e, StickShiftFrame *frame) {
    Streamer *s = user;
    StickShiftQuad q = {0, 0, 10, 10, 0, 0, 1, 1, {1, 1, 1, 1}, STICKSHIFT_FILL_SOLID, {0}};
    stickshift_frame_draw_quad(frame, &q);
    StickShiftQueryTerm tagged = {s->slot_tag, STICKSHIFT_QUERY_WITH};
    stickshift_world_query_count(e, &tagged, 1, &s->count_in_render);
    StickShiftEntity entity;
    s->spawn_in_render = stickshift_world_spawn(e, NULL, 0, &entity);
}

static int stream_game(void) {
    StickShiftContextDesc ctx_desc = {STICKSHIFT_API_VERSION, STICKSHIFT_BACKEND_NULL, NULL,
                                      1280, 720, 1.0f};
    StickShiftContext *ctx = NULL;
    CHECK(stickshift_context_create(&ctx_desc, &ctx), STICKSHIFT_OK);
    Streamer s;
    memset(&s, 0, sizeof s);
    StickShiftEngineDesc d;
    memset(&d, 0, sizeof d);
    d.api_version = STICKSHIFT_API_VERSION;
    d.size = sizeof d;
    StickShiftGame g = {sizeof g, 0, &s, stream_init, stream_update, stream_render, NULL, NULL};
    StickShiftEngine *e = NULL;
    CHECK(stickshift_engine_create(ctx, &d, &g, &e), STICKSHIFT_OK);
    CHECK(stickshift_engine_init(e), STICKSHIFT_OK);
    REQUIRE(s.slot_tag.id == STICKSHIFT_FIRST_GAME_COMPONENT);
    StickShiftTickDesc tick = {sizeof tick, STICKSHIFT_CLOCK_FIXED_STEP, 0, NULL, 0};
    for (int i = 0; i < 3; i++) CHECK(stickshift_engine_tick(e, &tick, NULL), STICKSHIFT_OK);

    /* Slots 0..=10: the default 60 ahead at a 6 spacing. */
    REQUIRE(s.spawned == 11 && s.tracked == 11 && s.count_in_render == 11);
    CHECK(s.config_in_spawn, STICKSHIFT_ERROR_WRONG_STATE);
    CHECK(s.stream_in_spawn, STICKSHIFT_ERROR_WRONG_STATE);
    CHECK(s.track_in_update, STICKSHIFT_ERROR_WRONG_STATE);
    CHECK(s.spawn_in_render, STICKSHIFT_ERROR_WRONG_STATE);
    StickShiftQueryTerm terms[] = {
        {{STICKSHIFT_COMPONENT_TRANSFORM}, STICKSHIFT_QUERY_READ},
        {s.slot_tag, STICKSHIFT_QUERY_WITH},
    };
    uint32_t count = 0;
    CHECK(stickshift_world_query_count(e, &terms[1], 1, &count), STICKSHIFT_OK);
    REQUIRE(count == 11);

    /* A snapshot: the slots in spawn order, 6 apart along -Z. */
    StickShiftQueryRows rows;
    memset(&rows, 0, sizeof rows);
    rows.size = sizeof rows;
    CHECK(stickshift_world_query(e, terms, 2, &rows), STICKSHIFT_OK);
    REQUIRE(rows.count == 11 && rows.term_count == 2 && rows.offsets[1] == UINT32_MAX);
    for (uint32_t i = 0; i < rows.count; i++) {
        StickShiftTransform t;
        memcpy(&t, rows.data + i * rows.stride + rows.offsets[0], sizeof t);
        REQUIRE(t.position.z == -6.0f * (float)i && t.rotation.w == 1.0f);
    }

    /* Despawn one: it is gone, a second despawn finds nothing, and the snapshot is
     * unchanged. The scene still tracks it until its next pass. */
    StickShiftEntity first = rows.entities[0];
    uint8_t existed = 0;
    CHECK(stickshift_world_despawn(e, first, &existed), STICKSHIFT_OK);
    REQUIRE(existed == 1);
    CHECK(stickshift_world_despawn(e, first, &existed), STICKSHIFT_OK);
    REQUIRE(existed == 0);
    REQUIRE(rows.entities[0].bits == first.bits);
    CHECK(stickshift_world_query_release(e, rows.token), STICKSHIFT_OK);
    CHECK(stickshift_world_query_release(e, rows.token), STICKSHIFT_ERROR_INVALID_ARGUMENT);
    CHECK(stickshift_world_query_count(e, &terms[1], 1, &count), STICKSHIFT_OK);
    REQUIRE(count == 10);
    StickShiftTransform gone;
    CHECK(stickshift_world_get(e, first, (StickShiftComponent){STICKSHIFT_COMPONENT_TRANSFORM},
                               &gone, sizeof gone),
          STICKSHIFT_ABSENT);
    uint32_t streamed = 0, len = 0;
    CHECK(stickshift_scene_streamed_count(e, &streamed), STICKSHIFT_OK);
    CHECK(stickshift_world_len(e, &len), STICKSHIFT_OK);
    REQUIRE(streamed == 11 && len == 10);

    StickShiftFrameRecord record;
    memset(&record, 0, sizeof record);
    record.size = sizeof record;
    CHECK(stickshift_context_get_last_frame(ctx, &record), STICKSHIFT_OK);
    REQUIRE(record.quad_count == 1);
    CHECK(stickshift_engine_destroy(e), STICKSHIFT_OK);
    CHECK(stickshift_context_destroy(ctx), STICKSHIFT_OK);
    printf("c engine: streaming OK (%d spawned, %u left)\n", s.spawned, len);
    return 0;
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
    return stream_game();
}
