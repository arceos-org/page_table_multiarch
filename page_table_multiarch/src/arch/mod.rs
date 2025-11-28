#[cfg(any(target_arch = "x86_64", docsrs))] 
#[cfg_attr(docsrs, doc(cfg(target_arch = "x86_64")))]
pub mod x86_64;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64", docsrs))]
#[cfg_attr(docsrs, doc(cfg(any(target_arch = "riscv32", target_arch = "riscv64"))))]
pub mod riscv;

#[cfg(any(target_arch = "aarch64", docsrs))]
#[cfg_attr(docsrs, doc(cfg(target_arch = "aarch64")))]
pub mod aarch64;

#[cfg(any(target_arch = "loongarch64", docsrs))]
#[cfg_attr(docsrs, doc(cfg(target_arch = "loongarch64")))]
pub mod loongarch64;