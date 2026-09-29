// Metamod hooks. Hooks only record into the event arena; the core runs in StartFrame, map
// start/end and the `lb` command.
#include "hooks.h"

#include <cmath>
#include <cstdarg>
#include <string>
#include <vector>

#include "arena.h"
#include "capture.h"
#include "fake_client.h"
#include "host_api.h"
#include "registry.h"
#include "rehlds_bridge.h"

namespace lb {

namespace {

std::vector<LbClientSnapshot> g_clients;
std::vector<LbSelfSnapshot> g_selves;

LbVec3 v3(const vec3_t v) {
    return LbVec3{v[0], v[1], v[2]};
}

bool is_fake(const edict_t *ed) {
    return ed && (ed->v.flags & FL_FAKECLIENT);
}

LbFrameHeader make_header(uint32_t flags) {
    LbFrameHeader h{};
    h.struct_size = sizeof(LbFrameHeader);
    h.map_epoch = state().epoch;
    h.frame_no = state().frame_no;
    h.sim_time = state().sim_time;
    h.frame_time = state().frame_time;
    h.mono_ns = mono_ns();
    h.engine_time = gpGlobals->time;
    h.flags = flags;
    h.max_clients = static_cast<uint32_t>(gpGlobals->maxClients);
    h.num_edicts = static_cast<uint32_t>(gpGlobals->maxEntities);
    return h;
}

void build_snapshots() {
    g_clients.clear();
    g_selves.clear();
    const int max = state().max_clients < kMaxSlots ? state().max_clients : kMaxSlots;
    for (int i = 1; i <= max; i++) {
        edict_t *ed = edict_of(i);
        const SlotInfo &s = state().slots[i];
        LbClientSnapshot c{};
        c.slot = static_cast<uint8_t>(i);
        const bool present = ed && !ed->free && s.connected;
        if (!present) {
            c.state = LB_CLIENT_FREE;
            g_clients.push_back(c);
            continue;
        }
        c.state = ed->pvPrivateData ? (s.in_game ? LB_CLIENT_SPAWNED : LB_CLIENT_CONNECTED) : LB_CLIENT_CONNECTING;
        c.is_fake = is_fake(ed) ? 1 : 0;
        c.is_ours = s.ours ? 1 : 0;
        c.userid = s.userid;
        c.origin = v3(ed->v.origin);
        c.velocity = v3(ed->v.velocity);
        c.angles = v3(ed->v.angles);
        c.view_ofs = v3(ed->v.view_ofs);
        c.mins = v3(ed->v.mins);
        c.maxs = v3(ed->v.maxs);
        c.rendercolor = v3(ed->v.rendercolor);
        c.flags = static_cast<uint32_t>(ed->v.flags);
        c.effects = static_cast<uint32_t>(ed->v.effects);
        c.movetype = static_cast<uint8_t>(ed->v.movetype);
        c.solid = static_cast<uint8_t>(ed->v.solid);
        c.deadflag = static_cast<uint8_t>(ed->v.deadflag);
        c.waterlevel = static_cast<uint8_t>(ed->v.waterlevel);
        c.rendermode = static_cast<uint8_t>(ed->v.rendermode);
        c.renderfx = static_cast<uint8_t>(ed->v.renderfx);
        c.step_left = static_cast<uint8_t>(ed->v.iStepLeft);
        c.renderamt = ed->v.renderamt;
        c.frame = ed->v.frame;
        c.frags = ed->v.frags;
        c.sequence = ed->v.sequence;
        c.gaitsequence = ed->v.gaitsequence;
        c.weaponmodel_id = intern(lb_string(ed->v.weaponmodel));
        c.model_id = intern(lb_string(ed->v.model));
        if (!c.is_fake) {
            int ping = 0;
            int loss = 0;
            g_engfuncs.pfnGetPlayerStats(ed, &ping, &loss);
            c.ping_ms = static_cast<uint16_t>(ping < 0 ? 0 : ping);
        }
        g_clients.push_back(c);

        if (s.ours && !s.zombie && ed->pvPrivateData) {
            LbSelfSnapshot me{};
            me.slot = static_cast<uint8_t>(i);
            me.deadflag = static_cast<uint8_t>(ed->v.deadflag);
            me.movetype = static_cast<uint8_t>(ed->v.movetype);
            me.waterlevel = static_cast<uint8_t>(ed->v.waterlevel);
            me.bot_gen = s.bot_gen;
            me.health = ed->v.health;
            me.armor = ed->v.armorvalue;
            me.weapons_mask = static_cast<uint32_t>(ed->v.weapons);
            me.maxspeed = ed->v.maxspeed;
            me.fov = ed->v.fov;
            me.duck_time = ed->v.flDuckTime;
            me.fall_velocity = ed->v.flFallVelocity;
            me.flags = static_cast<uint32_t>(ed->v.flags);
            me.watertype = ed->v.watertype;
            me.origin = v3(ed->v.origin);
            me.velocity = v3(ed->v.velocity);
            me.v_angle = v3(ed->v.v_angle);
            me.angles = v3(ed->v.angles);
            me.punchangle = v3(ed->v.punchangle);
            me.view_ofs = v3(ed->v.view_ofs);
            me.basevelocity = v3(ed->v.basevelocity);
            me.in_duck = ed->v.bInDuck ? 1 : 0;
            const char *slj = g_engfuncs.pfnGetPhysicsKeyValue(ed, "slj");
            me.has_longjump = (slj && slj[0] == '1') ? 1 : 0;
            me.fixangle = static_cast<uint8_t>(ed->v.fixangle);
            me.groundentity = static_cast<uint16_t>(index_of(ed->v.groundentity));
            me.buttons_applied = static_cast<uint16_t>(ed->v.button);
            me.frags = ed->v.frags;
            g_selves.push_back(me);
        }
    }
}

void advance_clock() {
    State &s = state();
    double sim = 0.0;
    double dt = 0.0;
    if (rehlds_time(&sim, &dt)) {
        s.sim_time = sim;
        s.frame_time = dt;
        return;
    }
    dt = static_cast<double>(gpGlobals->frametime);
    const double engine = static_cast<double>(gpGlobals->time);
    double t = s.clock_valid ? s.sim_time + s.pending_dt : engine;
    if (std::fabs(t - engine) > 0.05) t = engine;
    s.sim_time = t;
    s.pending_dt = dt;
    s.frame_time = dt;
    s.clock_valid = true;
}

void call_frame_pre() {
    if (!state().core_ok || !state().map_active) return;
    state().frame_no++;
    advance_clock();
    emulate_network_duties();
    build_snapshots();
    arena().swap();
    LbFrameInput in{};
    in.struct_size = sizeof(LbFrameInput);
    in.header = make_header(LB_FRAME_PRE | (state().first_frame ? LB_FRAME_FIRST : 0));
    in.clients = g_clients.data();
    in.client_count = static_cast<uint32_t>(g_clients.size());
    in.selves = g_selves.data();
    in.self_count = static_cast<uint32_t>(g_selves.size());
    const std::vector<uint8_t> &events = arena().delivered();
    in.events.data = events.data();
    in.events.len = static_cast<uint32_t>(events.size());
    in.events.count = arena().delivered_count();
    in.events.dropped = arena().delivered_dropped();
    in.events.first_seq = arena().delivered_first_seq();
    state().core_depth++;
    {
        ArenaContext ctx(LB_CTX_CORE);
        lb_core_frame_pre(&in);
    }
    state().core_depth--;
    state().first_frame = false;
}

void call_frame_post() {
    if (!state().core_ok || !state().map_active) return;
    LbFrameInput in{};
    in.struct_size = sizeof(LbFrameInput);
    in.header = make_header(LB_FRAME_POST);
    state().core_depth++;
    {
        ArenaContext ctx(LB_CTX_CORE);
        lb_core_frame_post(&in);
    }
    state().core_depth--;
}

std::string info_value(edict_t *ed, const char *key) {
    char *info = g_engfuncs.pfnGetInfoKeyBuffer(ed);
    const char *v = info ? g_engfuncs.pfnInfoKeyValue(info, key) : nullptr;
    return v ? v : "";
}

}  // namespace

void begin_map(bool late) {
    state().clock_valid = false;
    state().max_clients = gpGlobals->maxClients;
    state().epoch++;
    state().map_active = true;
    state().first_frame = true;
    reset_interned();
    registry_clear();
    if (!state().core_ok) return;
    const char *map = lb_string(gpGlobals->mapname);
    const std::string bsp = std::string("maps/") + map + ".bsp";
    LbMapInfo info{};
    info.struct_size = sizeof(LbMapInfo);
    info.map_epoch = state().epoch;
    info.map_name = str(map);
    info.bsp_path = str(bsp);
    info.max_clients = static_cast<uint32_t>(gpGlobals->maxClients);
    info.max_edicts = static_cast<uint32_t>(gpGlobals->maxEntities);
    info.late_load = late ? 1 : 0;
    state().core_depth++;
    lb_core_map_start(&info);
    state().core_depth--;
    registry_rescan();
}

void scan_existing_clients() {
    for (int i = 1; i <= gpGlobals->maxClients && i <= kMaxSlots; i++) {
        edict_t *ed = edict_of(i);
        if (!ed || ed->free || !(ed->v.flags & FL_CLIENT) || !ed->v.netname) continue;
        SlotInfo &s = state().slots[i];
        s.connected = true;
        s.in_game = ed->pvPrivateData != nullptr;
        s.userid = g_engfuncs.pfnGetPlayerUserId(ed);
        const std::string name = info_value(ed, "name");
        const std::string model = info_value(ed, "model");
        record_client(LB_CLIENT_EV_CONNECT, i, name.c_str(), model.c_str(), "", "");
        if (s.in_game) record_client(LB_CLIENT_EV_PUT_IN_SERVER, i, name.c_str(), model.c_str(), "", g_engfuncs.pfnGetPlayerAuthId(ed));
    }
}

// ------------------------------------------------------------------------------------------------
// DLL API
// ------------------------------------------------------------------------------------------------

LB_ENTRY int h_Spawn(edict_t *ent) {
    if (ent && std::strcmp(lb_string(ent->v.classname), "worldspawn") == 0) {
        state().beam_sprite = g_engfuncs.pfnPrecacheModel("sprites/laserbeam.spr");
    }
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, 0);
}

LB_ENTRY int h_Spawn_Post(edict_t *ent) {
    if (state().map_active) registry_on_spawn(ent);
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, 0);
}

LB_ENTRY qboolean h_ClientConnect_Post(edict_t *ed, const char *name, const char *address, char *) {
    const qboolean accepted = *static_cast<qboolean *>(state().meta->orig_ret);
    const int slot = index_of(ed);
    if (slot >= 1 && slot <= kMaxSlots) {
        SlotInfo &s = state().slots[slot];
        if (state().creating_slot != slot) {
            s = SlotInfo{};
        }
        s.connected = accepted != 0;
        s.userid = g_engfuncs.pfnGetPlayerUserId(ed);
        const std::string model = info_value(ed, "model");
        record_client(accepted ? LB_CLIENT_EV_CONNECT : LB_CLIENT_EV_CONNECT_REJECTED, slot, name, model.c_str(), address, "");
    }
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, TRUE);
}

LB_ENTRY void h_ClientDisconnect(edict_t *ed) {
    const int slot = index_of(ed);
    if (slot >= 1 && slot <= kMaxSlots) {
        record_client(LB_CLIENT_EV_DISCONNECT, slot, lb_string(ed->v.netname), "", "", "");
        forget_slot(slot);
    }
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_ClientPutInServer_Post(edict_t *ed) {
    const int slot = index_of(ed);
    if (slot >= 1 && slot <= kMaxSlots) {
        SlotInfo &s = state().slots[slot];
        s.connected = true;
        s.in_game = true;
        s.userid = g_engfuncs.pfnGetPlayerUserId(ed);
        const std::string name = info_value(ed, "name");
        const std::string model = info_value(ed, "model");
        record_client(LB_CLIENT_EV_PUT_IN_SERVER, slot, name.c_str(), model.c_str(), "", g_engfuncs.pfnGetPlayerAuthId(ed));
    }
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_ClientUserInfoChanged_Post(edict_t *ed, char *info) {
    const int slot = index_of(ed);
    if (slot >= 1 && slot <= kMaxSlots && info && state().slots[slot].connected) {
        record_client(LB_CLIENT_EV_INFO, slot, g_engfuncs.pfnInfoKeyValue(info, "name"),
                      g_engfuncs.pfnInfoKeyValue(info, "model"), "", "");
    }
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_ClientCommand(edict_t *ed) {
    if (fake_argv_active() || !ed || is_fake(ed)) LB_RETURN_META(mm::MRES_IGNORED);
    const int argc = g_engfuncs.pfnCmd_Argc();
    const char *cmd = g_engfuncs.pfnCmd_Argv(0);
    if (!cmd) LB_RETURN_META(mm::MRES_IGNORED);
    const bool is_lb = std::strcmp(cmd, "lb") == 0;
    const bool is_say = std::strcmp(cmd, "say") == 0 || std::strcmp(cmd, "say_team") == 0;
    if (!is_lb && !is_say) LB_RETURN_META(mm::MRES_IGNORED);
    std::vector<std::string> args;
    std::vector<const char *> argv;
    for (int i = 0; i < argc && i < 32; i++) args.emplace_back(g_engfuncs.pfnCmd_Argv(i));
    for (const std::string &a : args) argv.push_back(a.c_str());
    const int slot = index_of(ed);
    record_command(LB_EV_CLIENT_CMD, slot, g_engfuncs.pfnGetPlayerUserId(ed), static_cast<int>(argv.size()), argv.data(),
                   g_engfuncs.pfnCmd_Args());
    if (is_lb) LB_RETURN_META(mm::MRES_SUPERCEDE);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_ServerActivate_Post(edict_t *, int, int) {
    begin_map(false);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_ServerDeactivate() {
    if (state().map_active && state().core_ok) {
        state().core_depth++;
        lb_core_map_end(state().epoch);
        state().core_depth--;
    }
    state().map_active = false;
    registry_clear();
    // Engines drop every fake client on a level change; Metamod-FWGS never forwards their ClientDisconnect.
    for (int i = 1; i <= kMaxSlots; i++) {
        if (state().slots[i].ours) forget_slot(i);
    }
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_StartFrame() {
    if (state().map_active) registry_scan_new();
    call_frame_pre();
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_StartFrame_Post() {
    call_frame_post();
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_CmdStart(const edict_t *player, const struct usercmd_s *cmd, unsigned int random_seed) {
    uint32_t seed = 0;
    if (take_pending_seed(player, &seed) && state().gamedll && state().gamedll->dllapi_table->pfnCmdStart) {
        state().gamedll->dllapi_table->pfnCmdStart(player, cmd, seed);
        LB_RETURN_META(mm::MRES_SUPERCEDE);
    }
    (void)random_seed;
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_Sys_Error(const char *message) {
    if (state().core_ok) lb_core_fatal(str(message));
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_OnFreeEntPrivateData(edict_t *ent) {
    if (state().map_active) registry_on_free(ent);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void h_GameShutdown() {
    core_shutdown(LB_SHUTDOWN_PROCESS_EXIT);
    LB_RETURN_META(mm::MRES_IGNORED);
}

// ------------------------------------------------------------------------------------------------
// Engine API
// ------------------------------------------------------------------------------------------------

LB_ENTRY void e_MessageBegin(int dest, int type, const float *origin, edict_t *ed) {
    capture_message_begin(dest, type, origin, ed);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_MessageEnd_Post() {
    capture_message_end();
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteByte(int v) {
    capture_arg_int(LB_MSG_ARG_BYTE, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteChar(int v) {
    capture_arg_int(LB_MSG_ARG_CHAR, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteShort(int v) {
    capture_arg_int(LB_MSG_ARG_SHORT, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteLong(int v) {
    capture_arg_int(LB_MSG_ARG_LONG, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteAngle(float v) {
    capture_arg_float(LB_MSG_ARG_ANGLE, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteCoord(float v) {
    capture_arg_float(LB_MSG_ARG_COORD, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteString(const char *v) {
    capture_arg_string(v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_WriteEntity(int v) {
    capture_arg_int(LB_MSG_ARG_ENTITY, v);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY int e_RegUserMsg_Post(const char *name, int) {
    const int id = *static_cast<int *>(state().meta->orig_ret);
    record_reg_msg(id, name);
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, 0);
}

LB_ENTRY unsigned short e_PrecacheEvent_Post(int, const char *name) {
    const unsigned short index = *static_cast<unsigned short *>(state().meta->orig_ret);
    record_precache_event(index, name);
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, 0);
}

LB_ENTRY void e_PlaybackEvent(int flags, const edict_t *invoker, unsigned short event_index, float delay,
                              const float *origin, const float *angles, float fparam1, float fparam2, int iparam1,
                              int iparam2, int bparam1, int bparam2) {
    record_playback(flags, invoker, event_index, delay, origin, angles, fparam1, fparam2, iparam1, iparam2, bparam1,
                    bparam2);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_EmitSound(edict_t *ent, int channel, const char *sample, float volume, float attn, int flags, int pitch) {
    if (!rehlds_sound_channel()) record_sound(LB_SOUND_SRC_EMIT, ent, channel, sample, nullptr, volume, attn, flags, pitch);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_EmitAmbientSound(edict_t *ent, const float *pos, const char *sample, float volume, float attn, int flags,
                                 int pitch) {
    record_sound(LB_SOUND_SRC_AMBIENT, ent, 0, sample, pos, volume, attn, flags, pitch);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_ClientCommand(edict_t *ed, const char *, ...) {
    if (is_fake(ed)) LB_RETURN_META(mm::MRES_SUPERCEDE);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY void e_ClientPrintf(edict_t *ed, PRINT_TYPE, const char *) {
    if (is_fake(ed)) LB_RETURN_META(mm::MRES_SUPERCEDE);
    LB_RETURN_META(mm::MRES_IGNORED);
}

LB_ENTRY const char *e_Cmd_Args() {
    if (fake_argv_active()) LB_RETURN_META_VALUE(mm::MRES_SUPERCEDE, fake_args());
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, nullptr);
}

LB_ENTRY const char *e_Cmd_Argv(int i) {
    if (fake_argv_active()) LB_RETURN_META_VALUE(mm::MRES_SUPERCEDE, fake_argv(i));
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, nullptr);
}

LB_ENTRY int e_Cmd_Argc() {
    if (fake_argv_active()) LB_RETURN_META_VALUE(mm::MRES_SUPERCEDE, fake_argc());
    LB_RETURN_META_VALUE(mm::MRES_IGNORED, 0);
}

// ------------------------------------------------------------------------------------------------
// `lb` server command
// ------------------------------------------------------------------------------------------------

LB_ENTRY void cmd_lb() {
    const int argc = g_engfuncs.pfnCmd_Argc();
    std::vector<std::string> args;
    for (int i = 0; i < argc && i < 32; i++) args.emplace_back(g_engfuncs.pfnCmd_Argv(i));
    const char *line = g_engfuncs.pfnCmd_Args();
    if (!state().core_ok) {
        log_console("lambdabots: core is not running");
        return;
    }
    if (state().core_depth > 0) {
        std::vector<const char *> argv;
        for (const std::string &a : args) argv.push_back(a.c_str());
        record_command(LB_EV_SERVER_CMD, 0, 0, static_cast<int>(argv.size()), argv.data(), line);
        return;
    }
    std::vector<LbStr> argv;
    for (const std::string &a : args) argv.push_back(str(a));
    LbArgs a{};
    a.argc = static_cast<uint32_t>(argv.size());
    a.argv = argv.data();
    a.line = str(line ? line : "");
    state().core_depth++;
    lb_core_server_command(&a);
    state().core_depth--;
}

}  // namespace lb
