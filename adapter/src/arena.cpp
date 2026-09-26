#include "arena.h"

#include <cstring>

#include "state.h"

namespace lb {

namespace {
constexpr size_t kCapacity = 256 * 1024;
constexpr size_t kNonCriticalLimit = kCapacity * 3 / 4;
constexpr size_t kOverflowReserve = 64;
}  // namespace

Arena::Arena() {
    back_.reserve(kCapacity + kOverflowReserve);
    front_.reserve(kCapacity + kOverflowReserve);
}

Arena &arena() {
    static Arena a;
    return a;
}

bool Arena::push(uint16_t kind, bool critical, std::initializer_list<Part> parts) {
    size_t body = 0;
    for (const Part &p : parts) body += p.size;
    const size_t size = (sizeof(LbEventHeader) + body + 7) & ~size_t(7);
    const size_t limit = critical ? kCapacity : kNonCriticalLimit;
    if (size > 0xFFFF || back_.size() + size > limit) {
        back_dropped_++;
        back_dropped_bytes_ += static_cast<uint32_t>(size);
        return false;
    }
    LbEventHeader h{};
    h.kind = kind;
    h.size = static_cast<uint16_t>(size);
    h.seq = seq_++;
    h.ctx = ctx_;
    h.frame_no_low = static_cast<uint32_t>(state().frame_no);
    h.sim_time = gpGlobals ? static_cast<double>(gpGlobals->time) : 0.0;
    if (back_count_ == 0) back_first_seq_ = h.seq;
    const size_t start = back_.size();
    back_.resize(start + size, 0);
    std::memcpy(back_.data() + start, &h, sizeof(h));
    size_t off = start + sizeof(h);
    for (const Part &p : parts) {
        if (p.size) std::memcpy(back_.data() + off, p.data, p.size);
        off += p.size;
    }
    back_count_++;
    return true;
}

void Arena::clear() {
    back_.clear();
    front_.clear();
    back_count_ = back_dropped_ = back_dropped_bytes_ = back_first_seq_ = 0;
    front_count_ = front_dropped_ = front_first_seq_ = 0;
    ctx_ = LB_CTX_FRAME;
}

void Arena::swap() {
    if (back_dropped_) {
        LbEvOverflow o{back_dropped_, back_dropped_bytes_};
        const size_t size = (sizeof(LbEventHeader) + sizeof(o) + 7) & ~size_t(7);
        LbEventHeader h{};
        h.kind = LB_EV_OVERFLOW;
        h.size = static_cast<uint16_t>(size);
        h.seq = seq_++;
        h.ctx = LB_CTX_FRAME;
        const size_t start = back_.size();
        back_.resize(start + size, 0);
        std::memcpy(back_.data() + start, &h, sizeof(h));
        std::memcpy(back_.data() + start + sizeof(h), &o, sizeof(o));
        back_count_++;
    }
    front_.swap(back_);
    front_count_ = back_count_;
    front_dropped_ = back_dropped_;
    front_first_seq_ = back_first_seq_;
    back_.clear();
    back_count_ = 0;
    back_dropped_ = 0;
    back_dropped_bytes_ = 0;
    back_first_seq_ = 0;
}

}  // namespace lb
