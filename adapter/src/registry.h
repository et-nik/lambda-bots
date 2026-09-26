// Tracked entity registry: classification by core-supplied rules, snapshots on demand.
#pragma once

#include <cstdint>

#include "state.h"

namespace lb {

int32_t registry_set_rules(const LbTrackRule *rules, uint32_t count);
void registry_on_spawn(edict_t *ent);
void registry_on_free(edict_t *ent);
void registry_rescan();
void registry_clear();
uint32_t registry_snapshot(uint32_t kind_mask, LbEntitySnapshot *out, uint32_t cap);
int32_t registry_get(LbEntRef ref, LbEntitySnapshot *out);
void fill_entity_snapshot(edict_t *ed, uint8_t kind, LbEntitySnapshot *out);

}  // namespace lb
