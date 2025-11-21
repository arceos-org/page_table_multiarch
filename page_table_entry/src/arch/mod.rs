#[cfg(any(target_arch = "x86_64", feature = "all"))]
pub mod x86_64;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64", feature = "all"))]
pub mod riscv;

#[cfg(any(target_arch = "aarch64", feature = "all"))]
pub mod aarch64;

#[cfg(any(target_arch = "loongarch64", feature = "all"))]
pub mod loongarch64;
