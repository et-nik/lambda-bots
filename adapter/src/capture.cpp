#include "capture.h"

#include <unordered_map>
#include <vector>

#include "arena.h"

namespace lb {

namespace {

struct MsgCapture {
    bool active = false;
    bool own_send = false;
    uint8_t mask[32] = {};
    uint8_t msgmgr_mask[32] = {};
    LbEvUserMsg head{};
    std::vector<LbMsgArg> args;
    std::string strings;
};

MsgCapture &cap() {
    static MsgCapture c;
    return c;
}

struct Interned {
    std::unordered_map<std::string, uint16_t> by_text;
    // Engine strings live in its string pool for the whole map: most lookups hit the same pointer again, and a
    // pointer lookup does not build a std::string. The text is still compared, in case a pointer was reused.
    std::unordered_map<const char *, uint16_t> by_ptr;
    std::vector<const std::string *> texts{nullptr};
};

Interned &interned() {
    static Interned m;
    return m;
}

bool in_mask(const uint8_t *mask, int type) {
    return type >= 0 && type < 256 && (mask[type / 8] & (1u << (type % 8)));
}

bool broadcast_dest(int dest) {
    return dest == MSG_BROADCAST || dest == MSG_ALL || dest == MSG_PVS || dest == MSG_PAS || dest == MSG_PVS_R ||
           dest == MSG_PAS_R || dest == MSG_INIT || dest == MSG_SPEC;
}

LbVec3 vec(const float *v) {
    return v ? LbVec3{v[0], v[1], v[2]} : LbVec3{0, 0, 0};
}

void finish(bool truncated) {
    MsgCapture &c = cap();
    if (!c.active) return;
    c.active = false;
    if (c.args.size() > 255) return;
    c.head.argc = static_cast<uint8_t>(c.args.size());
    c.head.strings_len = static_cast<uint16_t>(c.strings.size());
    if (truncated) c.head.flags |= LB_MSG_FLAG_TRUNCATED;
    const bool critical = c.head.target_slot != 0 || c.head.dest == MSG_ALL;
    arena().push(LB_EV_USER_MSG, critical,
                 {{&c.head, sizeof(c.head)},
                  {c.args.data(), c.args.size() * sizeof(LbMsgArg)},
                  {c.strings.data(), c.strings.size()}});
}

}  // namespace

LbEntRef ent_ref(const edict_t *ed) {
    LbEntRef r{};
    if (!ed) return r;
    r.index = static_cast<uint16_t>(index_of(ed));
    r.serial = static_cast<uint32_t>(ed->serialnumber);
    return r;
}

void capture_set_mask(const uint8_t mask[32]) {
    std::memcpy(cap().mask, mask, 32);
}

void capture_set_msgmgr_mask(const uint8_t mask[32]) {
    std::memcpy(cap().msgmgr_mask, mask, 32);
}

void capture_set_own_send(bool own) {
    cap().own_send = own;
}

void capture_message_begin(MsgSource source, int dest, int type, const float *origin, edict_t *ed) {
    MsgCapture &c = cap();
    if (c.active) finish(true);
    if (c.own_send || !state().core_ok || !in_mask(c.mask, type)) return;
    const bool from_msgmgr = source == MsgSource::MessageManager;
    if (!from_msgmgr && in_mask(c.msgmgr_mask, type)) return;
    uint8_t target = 0;
    if (dest == MSG_ONE || dest == MSG_ONE_UNRELIABLE) {
        if (!is_our_bot(ed)) return;
        target = static_cast<uint8_t>(index_of(ed));
    } else if (!broadcast_dest(dest)) {
        return;
    }
    c.active = true;
    c.head = LbEvUserMsg{};
    c.head.msg_id = type;
    c.head.dest = static_cast<uint8_t>(dest);
    c.head.target_slot = target;
    if (from_msgmgr) c.head.flags |= LB_MSG_FLAG_FROM_MSGMGR;
    if (origin) {
        c.head.flags |= LB_MSG_FLAG_HAS_ORIGIN;
        c.head.origin = vec(origin);
    }
    c.args.clear();
    c.strings.clear();
}

void capture_arg_int(uint8_t tag, int value) {
    MsgCapture &c = cap();
    if (!c.active) return;
    LbMsgArg a{};
    a.tag = tag;
    a.ival = value;
    c.args.push_back(a);
}

void capture_arg_float(uint8_t tag, float value) {
    MsgCapture &c = cap();
    if (!c.active) return;
    LbMsgArg a{};
    a.tag = tag;
    a.fval = value;
    c.args.push_back(a);
}

void capture_arg_string(const char *value) {
    MsgCapture &c = cap();
    if (!c.active) return;
    const size_t len = value ? std::strlen(value) : 0;
    if (c.strings.size() + len > 0xFFFF) {
        c.active = false;
        return;
    }
    LbMsgArg a{};
    a.tag = LB_MSG_ARG_STRING;
    a.str_off = static_cast<uint16_t>(c.strings.size());
    a.str_len = static_cast<uint16_t>(len);
    if (len) c.strings.append(value, len);
    c.args.push_back(a);
}

void capture_message_end() {
    finish(false);
}

static void record_named(uint16_t kind, int id, uint16_t extra, const char *text, bool critical) {
    const size_t len = text ? std::strlen(text) : 0;
    LbEvNamed n{id, static_cast<uint16_t>(len > 0xFFFF ? 0xFFFF : len), extra};
    arena().push(kind, critical, {{&n, sizeof(n)}, {text, n.len}});
}

void record_reg_msg(int id, const char *name) {
    record_named(LB_EV_REG_MSG, id, 0, name, true);
}

void record_precache_event(int index, const char *name) {
    record_named(LB_EV_PRECACHE_EVENT, index, 0, name, true);
}

uint16_t intern(const char *s) {
    if (!s || !*s) return 0;
    auto &m = interned();
    auto p = m.by_ptr.find(s);
    if (p != m.by_ptr.end() && *m.texts[p->second] == s) return p->second;
    auto it = m.by_text.find(s);
    uint16_t id = 0;
    if (it != m.by_text.end()) {
        id = it->second;
    } else {
        if (m.by_text.size() >= 0xFFFE) return 0;
        id = static_cast<uint16_t>(m.by_text.size() + 1);
        it = m.by_text.emplace(s, id).first;
        m.texts.push_back(&it->first);
        record_named(LB_EV_STRING, id, 0, s, true);
    }
    m.by_ptr[s] = id;
    return id;
}

void reset_interned() {
    auto &m = interned();
    m.by_ptr.clear();
    m.by_text.clear();
    m.texts.assign(1, nullptr);
}

void record_client(uint8_t what, int slot, const char *name, const char *model, const char *addr, const char *auth) {
    LbEvClient c{};
    c.what = what;
    c.slot = static_cast<uint8_t>(slot);
    if (slot >= 1 && slot <= kMaxSlots) {
        const SlotInfo &s = state().slots[slot];
        c.is_ours = s.ours ? 1 : 0;
        c.userid = s.userid;
        c.bot_gen = s.bot_gen;
    }
    edict_t *ed = slot >= 1 ? edict_of(slot) : nullptr;
    c.is_fake = (ed && (ed->v.flags & FL_FAKECLIENT)) ? 1 : 0;
    if (ed && c.userid == 0) c.userid = g_engfuncs.pfnGetPlayerUserId(ed);
    auto clamp = [](const char *s) -> uint8_t {
        const size_t n = s ? std::strlen(s) : 0;
        return static_cast<uint8_t>(n > 255 ? 255 : n);
    };
    c.name_len = clamp(name);
    c.model_len = clamp(model);
    c.addr_len = clamp(addr);
    c.auth_id_len = clamp(auth);
    arena().push(LB_EV_CLIENT, true,
                 {{&c, sizeof(c)}, {name, c.name_len}, {model, c.model_len}, {addr, c.addr_len}, {auth, c.auth_id_len}});
}

void record_command(uint16_t kind, int slot, int userid, int argc, const char *const *argv, const char *line) {
    if (argc < 0) argc = 0;
    if (argc > 32) argc = 32;
    LbEvCommand c{};
    c.slot = static_cast<uint8_t>(slot);
    c.argc = static_cast<uint8_t>(argc);
    const size_t line_len = line ? std::strlen(line) : 0;
    c.line_len = static_cast<uint16_t>(line_len > 4096 ? 4096 : line_len);
    c.userid = userid;
    std::string body;
    for (int i = 0; i < argc; i++) {
        const char *a = argv[i] ? argv[i] : "";
        const size_t n = std::strlen(a);
        const uint16_t len = static_cast<uint16_t>(n > 1024 ? 1024 : n);
        body.append(reinterpret_cast<const char *>(&len), sizeof(len));
        body.append(a, len);
    }
    arena().push(kind, true, {{&c, sizeof(c)}, {body.data(), body.size()}, {line, c.line_len}});
}

void record_sound(uint8_t source, edict_t *ent, int channel, const char *sample, const float *origin, float volume,
                  float attn, int flags, int pitch) {
    if (!state().core_ok || !sample) return;
    LbEvSound s{};
    s.source = source;
    s.channel = static_cast<uint8_t>(channel);
    const size_t len = std::strlen(sample);
    s.sample_len = static_cast<uint16_t>(len > 255 ? 255 : len);
    s.entity = ent_ref(ent);
    if (origin) {
        s.origin = vec(origin);
    } else if (ent) {
        const float c[3] = {(ent->v.absmin[0] + ent->v.absmax[0]) * 0.5f, (ent->v.absmin[1] + ent->v.absmax[1]) * 0.5f,
                            (ent->v.absmin[2] + ent->v.absmax[2]) * 0.5f};
        s.origin = vec(c);
    }
    s.volume = volume;
    s.attenuation = attn;
    s.flags = flags;
    s.pitch = pitch;
    arena().push(LB_EV_SOUND, false, {{&s, sizeof(s)}, {sample, s.sample_len}});
}

void record_playback(int flags, const edict_t *invoker, unsigned short event_index, float delay, const float *origin,
                     const float *angles, float fparam1, float fparam2, int iparam1, int iparam2, int bparam1,
                     int bparam2) {
    if (!state().core_ok) return;
    LbEvPlayback p{};
    p.flags = flags;
    p.event_index = event_index;
    p.invoker = ent_ref(invoker);
    p.origin = vec(origin);
    p.angles = vec(angles);
    if (invoker) p.invoker_origin = vec(invoker->v.origin);
    p.delay = delay;
    p.fparam1 = fparam1;
    p.fparam2 = fparam2;
    p.iparam1 = iparam1;
    p.iparam2 = iparam2;
    p.bparam1 = bparam1;
    p.bparam2 = bparam2;
    arena().push(LB_EV_PLAYBACK, false, {{&p, sizeof(p)}});
}

void record_entity(uint8_t what, uint8_t kind, edict_t *ent) {
    LbEvEntity e{};
    e.what = what;
    e.kind = kind;
    e.classname_id = intern(ent ? lb_string(ent->v.classname) : "");
    e.ent = ent_ref(ent);
    if (ent) e.origin = vec(ent->v.origin);
    arena().push(LB_EV_ENTITY, what == LB_ENTITY_EV_FREE, {{&e, sizeof(e)}});
}

void record_fixangle(int slot, uint32_t bot_gen, uint8_t mode, const float *angles) {
    LbEvFixangle f{};
    f.slot = static_cast<uint8_t>(slot);
    f.mode = mode;
    f.bot_gen = bot_gen;
    f.angles = vec(angles);
    arena().push(LB_EV_FIXANGLE, true, {{&f, sizeof(f)}});
}

void record_log(int level, const std::string &text) {
    record_named(LB_EV_LOG, level, 0, text.c_str(), level <= 1);
}

}  // namespace lb
