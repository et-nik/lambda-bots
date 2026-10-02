//! Transcendental functions that give the same bits in every build and on every platform. `f32::sin` and the like
//! go to the platform's libm through LLVM intrinsics, and one build may turn a sine and a cosine into a single
//! `sincos` call where another does not: a replay by `lb-cli` then differs from the server in the last bit. These are
//! plain Rust (a port of musl's libm), compiled the same everywhere. The std versions are disallowed in `clippy.toml`.

pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

pub fn sin_cos(x: f32) -> (f32, f32) {
    libm::sincosf(x)
}

pub fn tan(x: f32) -> f32 {
    libm::tanf(x)
}

pub fn atan(x: f32) -> f32 {
    libm::atanf(x)
}

pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

pub fn asin(x: f32) -> f32 {
    libm::asinf(x)
}

pub fn acos(x: f32) -> f32 {
    libm::acosf(x)
}

pub fn exp(x: f32) -> f32 {
    libm::expf(x)
}

pub fn ln(x: f32) -> f32 {
    libm::logf(x)
}
