#include "aimlog.h"

#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <string>
#include <unordered_map>

#include "usercmd.h"

namespace lb {

namespace {

constexpr uint64_t kSampleNs = 4'000'000;
constexpr uint64_t kFlushNs = 1'000'000'000;

struct AimLog {
    bool enabled = false;
    FILE *out = nullptr;
    std::string path;
    uint64_t start_ns = 0;
    uint64_t next_sample = 0;
    uint64_t next_flush = 0;
    uint64_t lines = 0;
    std::unordered_map<unsigned short, std::string> events;
    int userid[kMaxSlots + 1] = {};
    std::string name[kMaxSlots + 1];
    // Sight from a human (first index) to another player as last written: bit 0 the eye, bit 1 the middle; -1 none.
    int8_t sight[kMaxSlots + 1][kMaxSlots + 1] = {};
};

AimLog &aim() {
    static AimLog l;
    return l;
}

double seconds() {
    return static_cast<double>(mono_ns() - aim().start_ns) * 1e-9;
}

edict_t *player(int slot) {
    edict_t *ed = edict_of(slot);
    if (!ed || ed->free || !(ed->v.flags & FL_CLIENT) || !ed->pvPrivateData || !ed->v.netname) return nullptr;
    return lb_string(ed->v.netname)[0] ? ed : nullptr;
}

bool alive(const edict_t *ed) {
    return ed->v.deadflag == DEAD_NO && ed->v.health > 0.0f;
}

std::string weapon(const edict_t *ed) {
    std::string w = lb_string(ed->v.weaponmodel);
    if (w.rfind("models/p_", 0) == 0) w.erase(0, 9);
    if (w.size() > 4 && w.compare(w.size() - 4, 4, ".mdl") == 0) w.resize(w.size() - 4);
    return w.empty() ? "-" : w;
}

void line(const char *fmt, ...) {
    AimLog &l = aim();
    va_list ap;
    va_start(ap, fmt);
    std::vfprintf(l.out, fmt, ap);
    va_end(ap);
    l.lines++;
}

void open_file() {
    AimLog &l = aim();
    if (l.out || !state().map_active) return;
    const char *map = lb_string(gpGlobals->mapname);
    char stamp[32];
    const std::time_t now = std::time(nullptr);
    std::tm local{};
#ifdef _WIN32
    localtime_s(&local, &now);
#else
    localtime_r(&now, &local);
#endif
    std::strftime(stamp, sizeof(stamp), "%Y%m%d-%H%M%S", &local);
    l.path = state().install_dir + "/logs/aim-" + map + "-" + stamp + ".tsv";
    l.out = std::fopen(l.path.c_str(), "w");
    if (!l.out) {
        log_console("[lambdabots] aimlog: cannot write %s", l.path.c_str());
        return;
    }
    std::setvbuf(l.out, nullptr, _IOFBF, 1 << 20);
    l.start_ns = mono_ns();
    l.next_sample = 0;
    l.next_flush = l.start_ns + kFlushNs;
    l.lines = 0;
    for (int i = 0; i <= kMaxSlots; i++) {
        l.userid[i] = 0;
        l.name[i].clear();
        std::memset(l.sight[i], -1, sizeof(l.sight[i]));
    }
    line("# lambdabots aimlog 1\tmap %s\tmaxplayers %d\n", map, gpGlobals->maxClients);
    line("# t: seconds on the server's monotonic clock since the file began; angles in degrees, pitch down positive\n");
    line("# P\tt\tslot\tuserid\tbot\tname\t(a player first seen or renamed)\n");
    line("# C\tt\tslot\tmsec\tpitch\tyaw\tbuttons\tforward\tside\tup\tweapon\tlerp_msec\t(a human's usercmd as run)\n");
    line("# S\tt\tslot\tdead\tx\ty\tz\tvx\tvy\tvz\tview_z\tpitch\tyaw\tflags\thealth\tarmor\tweapon\tping\n");
    line("# V\tt\thuman\tother\teye\tmiddle\t(sight from the human's eye changed; 1 = clear)\n");
    line("# E\tt\tslot\tevent\t(a weapon event)\n");
    line("# T\tt\tengine_time\n");
    log_console("[lambdabots] aimlog: writing %s", l.path.c_str());
}

void close_file() {
    AimLog &l = aim();
    if (!l.out) return;
    std::fclose(l.out);
    l.out = nullptr;
    log_console("[lambdabots] aimlog: %llu lines in %s", static_cast<unsigned long long>(l.lines), l.path.c_str());
}

void describe(int slot, const edict_t *ed, double t) {
    AimLog &l = aim();
    const int userid = g_engfuncs.pfnGetPlayerUserId(const_cast<edict_t *>(ed));
    const char *name = lb_string(ed->v.netname);
    if (userid == l.userid[slot] && l.name[slot] == name) return;
    l.userid[slot] = userid;
    l.name[slot] = name;
    line("P\t%.6f\t%d\t%d\t%d\t%s\n", t, slot, userid, (ed->v.flags & FL_FAKECLIENT) ? 1 : 0, name);
}

bool clear(const float *from, const float *to, edict_t *skip) {
    TraceResult tr{};
    g_engfuncs.pfnTraceLine(from, to, 1, skip, &tr);
    return tr.flFraction >= 1.0f && !tr.fStartSolid;
}

void sights(int viewer, edict_t *me, int max, double t) {
    AimLog &l = aim();
    const bool can_see = alive(me);
    vec3_t eye;
    for (int k = 0; k < 3; k++) eye[k] = me->v.origin[k] + me->v.view_ofs[k];
    for (int other = 1; other <= max; other++) {
        if (other == viewer) continue;
        edict_t *ed = player(other);
        int bits = -1;
        if (ed && can_see && alive(ed)) {
            vec3_t head;
            for (int k = 0; k < 3; k++) head[k] = ed->v.origin[k] + ed->v.view_ofs[k];
            bits = (clear(eye, head, me) ? 1 : 0) | (clear(eye, ed->v.origin, me) ? 2 : 0);
        }
        if (bits == l.sight[viewer][other]) continue;
        l.sight[viewer][other] = static_cast<int8_t>(bits);
        if (bits < 0) {
            line("V\t%.6f\t%d\t%d\t-\t-\n", t, viewer, other);
        } else {
            line("V\t%.6f\t%d\t%d\t%d\t%d\n", t, viewer, other, bits & 1, bits >> 1);
        }
    }
}

}  // namespace

LB_ENTRY void aimlog_command() {
    AimLog &l = aim();
    const char *arg = g_engfuncs.pfnCmd_Argc() > 1 ? g_engfuncs.pfnCmd_Argv(1) : "";
    if (std::strcmp(arg, "on") == 0) {
        l.enabled = true;
        open_file();
    } else if (std::strcmp(arg, "off") == 0) {
        l.enabled = false;
        close_file();
    } else if (l.out) {
        log_console("aimlog: %llu lines, %.0f s in %s", static_cast<unsigned long long>(l.lines), seconds(),
                    l.path.c_str());
    } else {
        log_console("aimlog: %s; usage: lb_aimlog on|off", l.enabled ? "on, waiting for a map" : "off");
    }
}

void aimlog_map_start() {
    if (aim().enabled) open_file();
}

void aimlog_map_end() {
    close_file();
}

void aimlog_frame() {
    AimLog &l = aim();
    if (!l.out) return;
    const uint64_t now = mono_ns();
    if (now < l.next_sample) return;
    l.next_sample = now - l.next_sample < kSampleNs ? l.next_sample + kSampleNs : now + kSampleNs;
    const double t = static_cast<double>(now - l.start_ns) * 1e-9;
    const int max = gpGlobals->maxClients < kMaxSlots ? gpGlobals->maxClients : kMaxSlots;
    for (int slot = 1; slot <= max; slot++) {
        edict_t *ed = player(slot);
        if (!ed) {
            l.userid[slot] = 0;
            l.name[slot].clear();
            continue;
        }
        describe(slot, ed, t);
        const bool bot = (ed->v.flags & FL_FAKECLIENT) != 0;
        int ping = 0;
        if (!bot) {
            int loss = 0;
            g_engfuncs.pfnGetPlayerStats(ed, &ping, &loss);
        }
        line("S\t%.6f\t%d\t%d\t%.2f\t%.2f\t%.2f\t%.1f\t%.1f\t%.1f\t%.1f\t%.4f\t%.4f\t%d\t%.0f\t%.0f\t%s\t%d\n", t, slot,
             alive(ed) ? 0 : 1, ed->v.origin[0], ed->v.origin[1], ed->v.origin[2], ed->v.velocity[0],
             ed->v.velocity[1], ed->v.velocity[2], ed->v.view_ofs[2], ed->v.v_angle[0], ed->v.v_angle[1],
             static_cast<int>(ed->v.flags), ed->v.health, ed->v.armorvalue, weapon(ed).c_str(), ping);
        if (!bot) sights(slot, ed, max, t);
    }
    if (now >= l.next_flush) {
        l.next_flush = now + kFlushNs;
        line("T\t%.6f\t%.3f\n", t, gpGlobals->time);
        std::fflush(l.out);
    }
}

void aimlog_cmd(const edict_t *ed, const usercmd_s *cmd) {
    AimLog &l = aim();
    if (!l.out || !ed || !cmd || (ed->v.flags & FL_FAKECLIENT)) return;
    line("C\t%.6f\t%d\t%d\t%.4f\t%.4f\t%d\t%.0f\t%.0f\t%.0f\t%d\t%d\n", seconds(), index_of(ed), cmd->msec,
         cmd->viewangles[0], cmd->viewangles[1], cmd->buttons, cmd->forwardmove, cmd->sidemove, cmd->upmove,
         cmd->weaponselect, cmd->lerp_msec);
}

void aimlog_event_name(unsigned short index, const char *name) {
    if (name) aim().events[index] = name;
}

void aimlog_playback(const edict_t *invoker, unsigned short event_index) {
    AimLog &l = aim();
    if (!l.out || !is_player_edict(invoker)) return;
    const auto it = l.events.find(event_index);
    line("E\t%.6f\t%d\t%s\n", seconds(), index_of(invoker), it == l.events.end() ? "?" : it->second.c_str());
}

}  // namespace lb
