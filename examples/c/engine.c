/*
 * A small game on the Stick Shift Engine, in C: a cube drives down an endless, streamed
 * road past spinning markers, steered with A and D, followed by the engine's chase camera,
 * and the world rebases its origin as it goes. Escape quits.
 *
 *   engine [frames]
 *
 * The game is four C callbacks; the engine runs the loop. The host here is a plain loop of
 * fixed-step ticks on a null context, with a scripted key press, so it runs anywhere and
 * prints what happened. A windowed host creates a layer context instead, calls
 * stickshift_engine_resume when its view appears, ticks from its display link with
 * STICKSHIFT_CLOCK_ENGINE, and forwards the view's events with stickshift_engine_push_events.
 *
 * Build it with scripts/test-c-api.sh, or by hand against the installed library:
 *
 *   cc -std=c11 engine.c $(pkg-config --cflags --libs stickshift) -o engine
 *
 * Copyright (c) 2026 Parsa Bagheri. Released under the Apache-2.0 License.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "stickshift.h"

/* The game's state, behind the hooks' user pointer. Hooks return nothing (but init), so
 * they keep the first failure for the host to report. */
typedef struct Game {
    StickShiftComponent spin;  /* a game component: a marker's turn rate, radians per second */
    StickShiftEntity player;
    StickShiftMesh cube;       /* every entity is drawn with it, scaled by its transform */
    float lane;
    int rebases;
    StickShiftResult error;
    char what[64];
} Game;

static void note(Game *game, const char *what, StickShiftResult result) {
    if (result < 0 && game->error == STICKSHIFT_OK) {
        game->error = result;
        snprintf(game->what, sizeof game->what, "%s", what);
    }
}

static StickShiftComponentValue value(uint32_t id, const void *data, uint32_t size) {
    StickShiftComponentValue v = {{id}, data, size};
    return v;
}

/* A unit cube, one quad per face, in the POS_NORMAL_UV layout so a draw's base colour
 * colours it. */
typedef struct Vertex {
    float position[3], normal[3], uv[2];
} Vertex;

static StickShiftResult create_cube(StickShiftContext *ctx, StickShiftMesh *out) {
    Vertex vertices[24];
    uint32_t indices[36];
    for (int face = 0; face < 6; face++) {
        int axis = face / 2;
        float sign = face % 2 ? -1.0f : 1.0f;
        /* u x v points along the face's normal, so each quad winds counter-clockwise. */
        int u = (axis + (face % 2 ? 2 : 1)) % 3, v = (axis + (face % 2 ? 1 : 2)) % 3;
        static const float corners[4][2] = {{-1, -1}, {1, -1}, {1, 1}, {-1, 1}};
        for (int c = 0; c < 4; c++) {
            Vertex *vertex = &vertices[face * 4 + c];
            memset(vertex, 0, sizeof *vertex);
            vertex->position[axis] = 0.5f * sign;
            vertex->position[u] = 0.5f * corners[c][0];
            vertex->position[v] = 0.5f * corners[c][1];
            vertex->normal[axis] = sign;
        }
        static const uint32_t quad[6] = {0, 1, 2, 0, 2, 3};
        for (int i = 0; i < 6; i++) indices[face * 6 + i] = (uint32_t)(face * 4) + quad[i];
    }
    StickShiftMeshDesc desc = {
        STICKSHIFT_VERTEX_LAYOUT_POS_NORMAL_UV,
        {vertices, sizeof vertices, NULL, NULL}, /* copied: no release function */
        {indices, sizeof indices, NULL, NULL},
    };
    return stickshift_mesh_create(ctx, &desc, NULL, out);
}

/* The stream pass calls this once per new slot along the road, from inside
 * stickshift_scene_stream. A spawn callback may only use the world and track entities. */
static void spawn_slot(void *user, StickShiftEngine *engine, int64_t slot, StickShiftVec3 at) {
    Game *game = user;
    StickShiftTransform road = {{at.x, -0.05f, at.z}, {0, 0, 0, 1}, {6.0f, 0.1f, 8.0f}};
    StickShiftRenderComponent grey = {{0.35f, 0.35f, 0.38f}, STICKSHIFT_RENDER_SHAPE_CUBE};
    StickShiftComponentValue tile[] = {
        value(STICKSHIFT_COMPONENT_TRANSFORM, &road, sizeof road),
        value(STICKSHIFT_COMPONENT_RENDER, &grey, sizeof grey),
        value(STICKSHIFT_COMPONENT_STATIC_TAG, NULL, 0),
    };
    StickShiftEntity entity;
    note(game, "spawn a tile", stickshift_world_spawn(engine, tile, 3, &entity));
    note(game, "track a tile", stickshift_stream_track(engine, entity));
    if (slot % 3 == 0) {
        float side = slot % 2 ? 4.0f : -4.0f;
        StickShiftTransform post = {{at.x + side, 1.0f, at.z}, {0, 0, 0, 1}, {0.6f, 2.0f, 0.6f}};
        StickShiftRenderComponent yellow = {{0.95f, 0.8f, 0.2f}, STICKSHIFT_RENDER_SHAPE_CUBE};
        float rate = 1.5f;
        StickShiftComponentValue marker[] = {
            value(STICKSHIFT_COMPONENT_TRANSFORM, &post, sizeof post),
            value(STICKSHIFT_COMPONENT_RENDER, &yellow, sizeof yellow),
            value(game->spin.id, &rate, sizeof rate),
        };
        note(game, "spawn a marker", stickshift_world_spawn(engine, marker, 3, &entity));
        note(game, "track a marker", stickshift_stream_track(engine, entity));
    }
}

static StickShiftResult init(void *user, StickShiftEngine *engine) {
    Game *game = user;
    StickShiftComponentDesc spin = {"example.Spin", sizeof(float), sizeof(float)};
    StickShiftResult r = stickshift_component_register(engine, &spin, &game->spin);
    if (r != STICKSHIFT_OK) return r;
    StickShiftContext *ctx;
    stickshift_engine_get_context(engine, &ctx);
    if ((r = create_cube(ctx, &game->cube)) != STICKSHIFT_OK) return r;

    StickShiftStreamingConfig config;
    stickshift_streaming_config_default(&config);
    config.spawn_interval = 8.0f;
    config.spawn_ahead = 120.0f;
    config.despawn_behind = 16.0f;
    config.rebase_threshold = 100.0f;
    if ((r = stickshift_scene_set_config(engine, &config)) != STICKSHIFT_OK) return r;

    StickShiftTransform start = {{0, 0.5f, 0}, {0, 0, 0, 1}, {1, 1, 1}};
    StickShiftRenderComponent red = {{0.85f, 0.15f, 0.1f}, STICKSHIFT_RENDER_SHAPE_CUBE};
    StickShiftComponentValue player[] = {
        value(STICKSHIFT_COMPONENT_TRANSFORM, &start, sizeof start),
        value(STICKSHIFT_COMPONENT_RENDER, &red, sizeof red),
        value(STICKSHIFT_COMPONENT_DYNAMIC_TAG, NULL, 0),
    };
    if ((r = stickshift_world_spawn(engine, player, 3, &game->player)) != STICKSHIFT_OK) return r;
    stickshift_camera_set_clip_planes(engine, 0.1f, 300.0f);
    return stickshift_scene_stream(engine, start.position, spawn_slot, game);
}

static void update(void *user, StickShiftEngine *engine, float dt) {
    Game *game = user;
    StickShiftInputState input = {.size = sizeof input};
    note(game, "read input", stickshift_input_get(engine, &input));
    int steer = !!(input.keys[STICKSHIFT_KEY_D] & STICKSHIFT_KEY_STATE_DOWN) -
                !!(input.keys[STICKSHIFT_KEY_A] & STICKSHIFT_KEY_STATE_DOWN);
    game->lane += (float)steer * 6.0f * dt;
    if (game->lane > 2.5f) game->lane = 2.5f;
    if (game->lane < -2.5f) game->lane = -2.5f;

    StickShiftComponent transform = {STICKSHIFT_COMPONENT_TRANSFORM};
    StickShiftTransform t;
    note(game, "read the player",
         stickshift_world_get(engine, game->player, transform, &t, sizeof t));
    t.position.x = game->lane;
    t.position.z -= 30.0f * dt;
    note(game, "move the player",
         stickshift_world_set(engine, game->player, transform, &t, sizeof t));

    /* Past the threshold every transform shifts back toward the origin. The camera holds
     * world coordinates too, so the game carries it through the same shift. */
    StickShiftVec3 offset;
    note(game, "rebase", stickshift_scene_maybe_rebase(engine, t.position, &offset));
    if (offset.x != 0 || offset.y != 0 || offset.z != 0) {
        game->rebases++;
        StickShiftCameraState camera = {.size = sizeof camera};
        stickshift_camera_get(engine, &camera);
        StickShiftVec3 eye = {camera.position.x + offset.x, camera.position.y + offset.y,
                              camera.position.z + offset.z};
        StickShiftVec3 target = {camera.target.x + offset.x, camera.target.y + offset.y,
                                 camera.target.z + offset.z};
        stickshift_camera_set_position(engine, eye);
        stickshift_camera_set_target(engine, target);
        note(game, "read the player",
             stickshift_world_get(engine, game->player, transform, &t, sizeof t));
    }

    StickShiftChase chase;
    stickshift_chase_init(&chase, 8.0f, 3.0f);
    chase.look_at_height = 1.0f;
    chase.half_life = 0.2f;
    StickShiftVec3 ahead = {0, 0, -1};
    note(game, "follow", stickshift_camera_follow(engine, &chase, t.position, ahead, dt));
    note(game, "stream", stickshift_scene_stream(engine, t.position, spawn_slot, game));

    /* Turn every marker. The rows are a snapshot, so writing the world while walking them
     * is fine. */
    StickShiftQueryTerm terms[] = {{transform, STICKSHIFT_QUERY_READ},
                                   {game->spin, STICKSHIFT_QUERY_READ}};
    StickShiftQueryRows rows = {.size = sizeof rows};
    note(game, "query the markers", stickshift_world_query(engine, terms, 2, &rows));
    for (uint32_t i = 0; i < rows.count; i++) {
        const uint8_t *row = rows.data + i * rows.stride;
        StickShiftTransform marker;
        float rate;
        memcpy(&marker, row + rows.offsets[0], sizeof marker);
        memcpy(&rate, row + rows.offsets[1], sizeof rate);
        marker.rotation = stickshift_quat_mul(stickshift_quat_from_rotation_y(rate * dt),
                                              marker.rotation);
        note(game, "turn a marker",
             stickshift_world_set(engine, rows.entities[i], transform, &marker, sizeof marker));
    }
    stickshift_world_query_release(engine, rows.token);
}

/* The engine opened `frame` and pushed the camera; it submits the frame when this returns.
 * The render hook only reads the world. */
static void render(void *user, StickShiftEngine *engine, StickShiftFrame *frame) {
    Game *game = user;
    StickShiftQueryTerm terms[] = {{{STICKSHIFT_COMPONENT_TRANSFORM}, STICKSHIFT_QUERY_READ},
                                   {{STICKSHIFT_COMPONENT_RENDER}, STICKSHIFT_QUERY_READ}};
    StickShiftQueryRows rows = {.size = sizeof rows};
    note(game, "query the scene", stickshift_world_query(engine, terms, 2, &rows));
    for (uint32_t i = 0; i < rows.count; i++) {
        const uint8_t *row = rows.data + i * rows.stride;
        StickShiftTransform t;
        StickShiftRenderComponent look;
        memcpy(&t, row + rows.offsets[0], sizeof t);
        memcpy(&look, row + rows.offsets[1], sizeof look);
        StickShiftMat4 model = stickshift_transform_to_mat4(t);
        StickShiftMaterial material = {
            {look.color[0], look.color[1], look.color[2], 1}, {0}, {0, 0, 0}};
        note(game, "draw", stickshift_frame_draw_mesh(frame, game->cube, &model, &material));
    }
    /* Not released: a snapshot taken in a hook ends with the hook. */
}

static int fail(const char *what, StickShiftResult result) {
    fprintf(stderr, "%s failed: %s (%s)\n", what, stickshift_result_name(result),
            stickshift_last_error_message());
    return 1;
}

static StickShiftResult press(StickShiftEngine *engine, StickShiftHostEventKind kind,
                              StickShiftKey key, uint32_t *requests) {
    StickShiftHostEvent event = {.kind = kind, .code = key};
    return stickshift_engine_push_events(engine, &event, 1, sizeof event, requests);
}

int main(int argc, char **argv) {
    int frames = argc > 1 ? atoi(argv[1]) : 600;
    if (frames < 2) frames = 2;

    StickShiftContextDesc ctx_desc = {STICKSHIFT_API_VERSION, STICKSHIFT_BACKEND_NULL, NULL,
                                      1280, 720, 1.0f};
    StickShiftContext *ctx;
    StickShiftResult r = stickshift_context_create(&ctx_desc, &ctx);
    if (r != STICKSHIFT_OK) return fail("stickshift_context_create", r);

    Game game = {0};
    StickShiftGame hooks = {sizeof hooks, 0, &game, init, update, render, NULL, NULL};
    StickShiftEngineDesc desc = {
        .api_version = STICKSHIFT_API_VERSION,
        .size = sizeof desc,
        .audio = STICKSHIFT_AUDIO_SILENT,
        .pacing = STICKSHIFT_FRAME_PACING_WAIT,
        .flags = STICKSHIFT_ENGINE_QUIT_ON_ESCAPE,
    };
    StickShiftEngine *engine;
    if ((r = stickshift_engine_create(ctx, &desc, &hooks, &engine)) != STICKSHIFT_OK)
        return fail("stickshift_engine_create", r);
    if ((r = stickshift_engine_init(engine)) != STICKSHIFT_OK)
        return fail("stickshift_engine_init", r);

    /* The host's loop: one tick per display refresh, here one fixed step each. Hold D for a
     * second, then press Escape on the last frame. */
    int ticks = 0;
    for (int frame = 0; frame < frames; frame++) {
        uint32_t requests = 0;
        if (frame == 60) r = press(engine, STICKSHIFT_HOST_KEY_DOWN, STICKSHIFT_KEY_D, &requests);
        if (frame == 120) r = press(engine, STICKSHIFT_HOST_KEY_UP, STICKSHIFT_KEY_D, &requests);
        if (frame == frames - 1)
            r = press(engine, STICKSHIFT_HOST_KEY_DOWN, STICKSHIFT_KEY_ESCAPE, &requests);
        if (r != STICKSHIFT_OK) return fail("stickshift_engine_push_events", r);
        if (requests & STICKSHIFT_REQUEST_EXIT) break;

        StickShiftTickDesc tick = {.size = sizeof tick, .clock = STICKSHIFT_CLOCK_FIXED_STEP};
        StickShiftTickResult result = {.size = sizeof result};
        if ((r = stickshift_engine_tick(engine, &tick, &result)) != STICKSHIFT_OK)
            return fail("stickshift_engine_tick", r);
        ticks++;
        if (result.requests & STICKSHIFT_REQUEST_EXIT) break;
    }
    if (game.error != STICKSHIFT_OK) return fail(game.what, game.error);

    StickShiftEngineState state = {.size = sizeof state};
    StickShiftCameraState camera = {.size = sizeof camera};
    StickShiftFrameRecord last;
    memset(&last, 0, sizeof last);
    last.size = sizeof last;
    uint32_t entities = 0, streamed = 0;
    StickShiftResult reads[] = {
        stickshift_engine_get_state(engine, &state),
        stickshift_camera_get(engine, &camera),
        stickshift_world_len(engine, &entities),
        stickshift_scene_streamed_count(engine, &streamed),
        stickshift_context_get_last_frame(ctx, &last),
    };
    for (size_t i = 0; i < sizeof reads / sizeof reads[0]; i++)
        if (reads[i] != STICKSHIFT_OK) return fail("reading the engine back", reads[i]);
    printf("%d ticks, %llu updates, %d rebases; %u entities (%u streamed); the last frame drew "
           "%u meshes; the camera is at (%.1f, %.1f, %.1f)\n",
           ticks, (unsigned long long)state.updates, game.rebases, entities, streamed,
           last.draw_count, camera.position.x, camera.position.y, camera.position.z);
    if (game.rebases == 0 || last.draw_count != entities) {
        fprintf(stderr, "the drive did not go as planned\n");
        return 1;
    }

    /* The engine first (it releases the game), then its context. */
    if ((r = stickshift_engine_destroy(engine)) != STICKSHIFT_OK)
        return fail("stickshift_engine_destroy", r);
    if ((r = stickshift_context_destroy(ctx)) != STICKSHIFT_OK)
        return fail("stickshift_context_destroy", r);
    return 0;
}
