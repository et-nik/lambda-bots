#pragma once

#include "state.h"

namespace lb {

const LbHostApi &host_api();
void fill_compat_facts(LbCompatFacts *out);

}  // namespace lb
