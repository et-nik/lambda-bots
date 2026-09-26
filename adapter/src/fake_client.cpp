#include "fake_client.h"

#include <cstring>
#include <string>
#include <vector>

#include "arena.h"
#include "capture.h"
#include "rehlds_bridge.h"

namespace lb {

namespace {

struct FakeArgv {
    bool active = false;
    std::vector<std::string> argv;
    std::string args;
};

FakeArgv &fa() {
    static FakeArgv f;
    return f;
}

bool slot_matches(uint8_t slot, uint32_t bot_gen) {
    if (slot < 1 || slot > state().max_clients || slot > kMaxSlots) return false;
    const SlotInfo &s = state().slots[slot];
    return s.ours && s.bot_gen == bot_gen && !s.zombie;
}

bool is_engine_kill(const LbClientCommand &c) {
    return c.argc == 1 && c.argv[0].len == 4 && std::memcmp(c.argv[0].ptr, "kill", 4) == 0;
}

void copy_reason(LbCreateBotResult *out, const char *reason) {
    std::strncpy(reinterpret_cast<char *>(out->reject_reason), reason ? reason : "", sizeof(out->reject_reason) - 1);
}

}  // namespace

bool fake_argv_active() {
    return fa().active;
}

int fake_argc() {
    return static_cast<int>(fa().argv.size());
}

const char *fake_argv(int i) {
    return (i >= 0 && i < static_cast<int>(fa().argv.size())) ? fa().argv[i].c_str() : "";
}

const char *fake_args() {
    return fa().args.c_str();
}

int32_t create_bot(const LbCreateBotRequest *req, LbCreateBotResult *out) {
    std::memset(out, 0, sizeof(*out));
    if (!state().map_active) {
        out->status = LB_ERR_BUSY;
        return LB_ERR_BUSY;
    }
    const std::string name(reinterpret_cast<const char *>(req->name.ptr), req->name.len);
    edict_t *ed = g_engfuncs.pfnCreateFakeClient(name.c_str());
    if (!ed) {
        out->status = LB_ERR_FULL;
        return LB_ERR_FULL;
    }
    const int slot = index_of(ed);
    if (slot < 1 || slot > kMaxSlots) {
        out->status = LB_ERR_INVALID;
        return LB_ERR_INVALID;
    }
    if (ed->pvPrivateData) g_engfuncs.pfnFreeEntPrivateData(ed);
    ed->pvPrivateData = nullptr;
    std::memset(&ed->v, 0, sizeof(ed->v));
    ed->v.pContainingEntity = ed;
    ed->v.flags = FL_FAKECLIENT | FL_CLIENT;
    ed->v.netname = g_engfuncs.pfnAllocString(name.c_str());

    char *info = g_engfuncs.pfnGetInfoKeyBuffer(ed);
    for (uint32_t i = 0; i < req->infokey_count; i++) {
        const std::string key(reinterpret_cast<const char *>(req->infokeys[i].key.ptr), req->infokeys[i].key.len);
        const std::string value(reinterpret_cast<const char *>(req->infokeys[i].value.ptr), req->infokeys[i].value.len);
        g_engfuncs.pfnSetClientKeyValue(slot, info, key.c_str(), value.c_str());
    }

    SlotInfo &s = state().slots[slot];
    s = SlotInfo{};
    s.ours = true;
    s.bot_gen = ++state().bot_gen_counter;
    state().creating_slot = slot;

    if (state().util && state().util->call_game_entity) {
        state().util->call_game_entity(&state().plugin_info, "player", &ed->v);
    }
    char reject[128] = {};
    const std::string addr(req->connect_addr.len ? std::string(reinterpret_cast<const char *>(req->connect_addr.ptr), req->connect_addr.len) : std::string("127.0.0.1"));
    const qboolean accepted = dll_for_calls()->pfnClientConnect(ed, name.c_str(), addr.c_str(), reject);
    if (!accepted) {
        state().creating_slot = -1;
        s.zombie = true;
        const int userid = g_engfuncs.pfnGetPlayerUserId(ed);
        char cmd[96];
        std::snprintf(cmd, sizeof(cmd), "kick #%d\n", userid);
        g_engfuncs.pfnServerCommand(cmd);
        copy_reason(out, reject[0] ? reject : "rejected by the game");
        out->status = LB_ERR_REJECTED;
        return LB_ERR_REJECTED;
    }
    dll_for_calls()->pfnClientPutInServer(ed);
    state().creating_slot = -1;
    ed->v.flags |= FL_FAKECLIENT;
    s.connected = true;
    s.in_game = true;
    s.userid = g_engfuncs.pfnGetPlayerUserId(ed);
    out->status = LB_OK;
    out->slot = static_cast<uint8_t>(slot);
    out->userid = s.userid;
    out->bot_gen = s.bot_gen;
    return LB_OK;
}

int32_t kick_bot(uint8_t slot, uint32_t bot_gen, LbStr reason) {
    if (!slot_matches(slot, bot_gen)) return LB_ERR_STALE;
    edict_t *ed = edict_of(slot);
    const int userid = g_engfuncs.pfnGetPlayerUserId(ed);
    if (userid <= 0 || userid != state().slots[slot].userid) return LB_ERR_STALE;
    std::string why(reinterpret_cast<const char *>(reason.ptr), reason.len);
    for (char &c : why) {
        if (c == '"' || c == ';' || c == '\n' || c == '\r') c = ' ';
    }
    if (rehlds_drop_client(slot, why.c_str())) return LB_OK;
    char cmd[160];
    std::snprintf(cmd, sizeof(cmd), "kick #%d \"%s\"\n", userid, why.c_str());
    g_engfuncs.pfnServerCommand(cmd);
    return LB_OK;
}

int32_t bot_client_commands(const LbClientCommand *cmds, uint32_t count) {
    int32_t status = LB_OK;
    for (uint32_t i = 0; i < count; i++) {
        const LbClientCommand &c = cmds[i];
        if (!slot_matches(c.slot, c.bot_gen) || c.argc == 0) {
            status = LB_ERR_STALE;
            continue;
        }
        if (is_engine_kill(c)) {
            // `kill` never reaches the game's ClientCommand: the engine itself calls ClientKill for a living player.
            edict_t *ed = edict_of(c.slot);
            if (ed->v.health > 0.0f) {
                ArenaContext ctx(LB_CTX_BOTCLCMD | (static_cast<uint32_t>(c.slot) << 8));
                dll_for_calls()->pfnClientKill(ed);
            }
            continue;
        }
        FakeArgv &f = fa();
        f.argv.clear();
        f.args.clear();
        for (uint8_t a = 0; a < c.argc && a < 8; a++) {
            f.argv.emplace_back(reinterpret_cast<const char *>(c.argv[a].ptr), c.argv[a].len);
            if (a > 0) {
                if (a > 1) f.args += ' ';
                f.args += f.argv.back();
            }
        }
        f.active = true;
        {
            ArenaContext ctx(LB_CTX_BOTCLCMD | (static_cast<uint32_t>(c.slot) << 8));
            dll_for_calls()->pfnClientCommand(edict_of(c.slot));
        }
        f.active = false;
    }
    return status;
}

int32_t run_player_moves(const LbBotCommand *cmds, uint32_t count, LbMoveFeedback *feedback) {
    for (uint32_t i = 0; i < count; i++) {
        const LbBotCommand &c = cmds[i];
        LbMoveFeedback &fb = feedback[i];
        std::memset(&fb, 0, sizeof(fb));
        fb.slot = c.slot;
        fb.frame_no = state().frame_no;
        if (!slot_matches(c.slot, c.bot_gen)) {
            fb.status = LB_MOVE_STALE;
            continue;
        }
        edict_t *ed = edict_of(c.slot);
        SlotInfo &s = state().slots[c.slot];
        if (c.flags & LB_CMD_SET_SEED) {
            s.pending_seed = c.random_seed;
            s.has_seed = true;
        }
        const float angles[3] = {c.view_angles.x, c.view_angles.y, c.view_angles.z};
        {
            ArenaContext ctx(LB_CTX_BOTCMD | (static_cast<uint32_t>(c.slot) << 8));
            g_engfuncs.pfnRunPlayerMove(ed, angles, c.forwardmove, c.sidemove, c.upmove, c.buttons, c.impulse, c.msec);
        }
        s.has_seed = false;
        fb.status = LB_MOVE_OK;
        fb.deadflag = static_cast<uint8_t>(ed->v.deadflag);
        fb.waterlevel = static_cast<uint8_t>(ed->v.waterlevel);
        fb.movetype = static_cast<uint8_t>(ed->v.movetype);
        fb.flags = static_cast<uint32_t>(ed->v.flags);
        fb.origin = LbVec3{ed->v.origin[0], ed->v.origin[1], ed->v.origin[2]};
        fb.velocity = LbVec3{ed->v.velocity[0], ed->v.velocity[1], ed->v.velocity[2]};
        fb.v_angle = LbVec3{ed->v.v_angle[0], ed->v.v_angle[1], ed->v.v_angle[2]};
        fb.health = ed->v.health;
    }
    return LB_OK;
}

bool take_pending_seed(const edict_t *player, uint32_t *seed) {
    const int slot = index_of(player);
    if (slot < 1 || slot > kMaxSlots) return false;
    SlotInfo &s = state().slots[slot];
    if (!s.ours || !s.has_seed) return false;
    *seed = s.pending_seed;
    s.has_seed = false;
    return true;
}

void emulate_fixangle() {
    for (int slot = 1; slot <= state().max_clients && slot <= kMaxSlots; slot++) {
        SlotInfo &s = state().slots[slot];
        if (!s.ours || s.zombie) continue;
        edict_t *ed = edict_of(slot);
        if (!ed || ed->free || !ed->pvPrivateData) continue;
        if (ed->v.fixangle == 1) {
            record_fixangle(slot, s.bot_gen, 1, ed->v.angles);
            ed->v.fixangle = 0;
        } else if (ed->v.fixangle == 2) {
            const float add[3] = {0.0f, ed->v.avelocity[1], 0.0f};
            record_fixangle(slot, s.bot_gen, 2, add);
            ed->v.avelocity[1] = 0.0f;
            ed->v.fixangle = 0;
        }
    }
}

void forget_slot(int slot) {
    if (slot >= 1 && slot <= kMaxSlots) state().slots[slot] = SlotInfo{};
}

}  // namespace lb
