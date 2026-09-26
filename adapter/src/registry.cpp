#include "registry.h"

#include <string>
#include <vector>

#include "capture.h"

namespace lb {

namespace {

struct Rule {
    std::string pattern;
    bool prefix;
    uint8_t kind;
};

struct Registry {
    std::vector<Rule> rules;
    std::vector<uint8_t> kinds;
};

Registry &reg() {
    static Registry r;
    return r;
}

uint8_t classify(const char *classname) {
    if (!classname || !*classname) return 0;
    for (const Rule &r : reg().rules) {
        if (r.prefix ? std::strncmp(classname, r.pattern.c_str(), r.pattern.size()) == 0 : r.pattern == classname) {
            return r.kind;
        }
    }
    return 0;
}

LbVec3 v3(const vec3_t v) {
    return LbVec3{v[0], v[1], v[2]};
}

bool valid(edict_t *ed) {
    return ed && !ed->free && ed->pvPrivateData;
}

}  // namespace

int32_t registry_set_rules(const LbTrackRule *rules, uint32_t count) {
    Registry &r = reg();
    r.rules.clear();
    for (uint32_t i = 0; i < count; i++) {
        const LbTrackRule &x = rules[i];
        r.rules.push_back(Rule{std::string(reinterpret_cast<const char *>(x.pattern.ptr), x.pattern.len),
                               x.match_mode == LB_TRACK_PREFIX, x.kind});
    }
    return LB_OK;
}

void registry_clear() {
    reg().kinds.clear();
}

void registry_on_spawn(edict_t *ent) {
    if (!valid(ent)) return;
    const int index = index_of(ent);
    if (index <= state().max_clients) return;
    const uint8_t kind = classify(lb_string(ent->v.classname));
    Registry &r = reg();
    if (static_cast<size_t>(index) >= r.kinds.size()) r.kinds.resize(index + 1, 0);
    if (r.kinds[index] == kind) return;
    r.kinds[index] = kind;
    if (kind && state().core_ok) record_entity(LB_ENTITY_EV_SPAWN, kind, ent);
}

void registry_on_free(edict_t *ent) {
    if (!ent) return;
    const int index = index_of(ent);
    Registry &r = reg();
    if (index <= 0 || static_cast<size_t>(index) >= r.kinds.size() || r.kinds[index] == 0) return;
    if (state().core_ok) record_entity(LB_ENTITY_EV_FREE, r.kinds[index], ent);
    r.kinds[index] = 0;
}

void registry_rescan() {
    Registry &r = reg();
    r.kinds.assign(static_cast<size_t>(gpGlobals->maxEntities) + 1, 0);
    for (int i = state().max_clients + 1; i < gpGlobals->maxEntities; i++) {
        registry_on_spawn(edict_of(i));
    }
}

void fill_entity_snapshot(edict_t *ed, uint8_t kind, LbEntitySnapshot *out) {
    std::memset(out, 0, sizeof(*out));
    out->ent = ent_ref(ed);
    out->owner = ent_ref(ed->v.owner);
    out->classname_id = intern(lb_string(ed->v.classname));
    out->model_id = intern(lb_string(ed->v.model));
    out->kind = kind;
    out->solid = static_cast<uint8_t>(ed->v.solid);
    out->movetype = static_cast<uint8_t>(ed->v.movetype);
    out->rendermode = static_cast<uint8_t>(ed->v.rendermode);
    out->origin = v3(ed->v.origin);
    out->angles = v3(ed->v.angles);
    out->velocity = v3(ed->v.velocity);
    out->avelocity = v3(ed->v.avelocity);
    out->absmin = v3(ed->v.absmin);
    out->absmax = v3(ed->v.absmax);
    out->rendercolor = v3(ed->v.rendercolor);
    out->effects = static_cast<uint32_t>(ed->v.effects);
    out->spawnflags = static_cast<uint32_t>(ed->v.spawnflags);
    out->frame = ed->v.frame;
    out->renderamt = ed->v.renderamt;
    out->renderfx = static_cast<uint8_t>(ed->v.renderfx);
    out->deadflag = static_cast<uint8_t>(ed->v.deadflag);
}

uint32_t registry_snapshot(uint32_t kind_mask, LbEntitySnapshot *out, uint32_t cap) {
    Registry &r = reg();
    uint32_t n = 0;
    for (size_t i = 1; i < r.kinds.size() && n < cap; i++) {
        const uint8_t kind = r.kinds[i];
        if (!kind || !(kind_mask & (1u << kind))) continue;
        edict_t *ed = edict_of(static_cast<int>(i));
        if (!valid(ed)) continue;
        fill_entity_snapshot(ed, kind, &out[n++]);
    }
    return n;
}

int32_t registry_get(LbEntRef ref, LbEntitySnapshot *out) {
    if (ref.index == 0 || ref.index >= static_cast<uint16_t>(gpGlobals->maxEntities)) return LB_ERR_NOT_FOUND;
    edict_t *ed = edict_of(ref.index);
    if (!valid(ed) || static_cast<uint32_t>(ed->serialnumber) != ref.serial) return LB_ERR_STALE;
    Registry &r = reg();
    const uint8_t kind = ref.index < r.kinds.size() ? r.kinds[ref.index] : 0;
    fill_entity_snapshot(ed, kind, out);
    return LB_OK;
}

}  // namespace lb
