# page_table_entry

[![Crates.io](https://img.shields.io/crates/v/page_table_entry)](https://crates.io/crates/page_table_entry)

This crate provides the definition of page table entry for various hardware
architectures.

Currently supported architectures and page table entry types:

- x86: [`x86_64::X64PTE`][1]
- ARM: [`aarch64::A64PTE`][2]
- RISC-V: [`riscv::Rv64PTE`][3]

All these types implement the [`GenericPTE`][4] trait, which provides unified
methods for manipulating various page table entries.

[1]: https://docs.rs/page_table_entry/latest/page_table_entry/x86_64/struct.X64PTE.html
[2]: https://docs.rs/page_table_entry/latest/page_table_entry/aarch64/struct.A64PTE.html
[3]: https://docs.rs/page_table_entry/latest/page_table_entry/riscv/struct.Rv64PTE.html
[4]: https://docs.rs/page_table_entry/latest/page_table_entry/trait.GenericPTE.html

## Examples (x86_64)

```rust
use memory_addr::PhysAddr;
use x86_64::structures::paging::page_table::PageTableFlags;
use page_table_entry::{GenericPTE, MappingFlags, x86_64::X64PTE};

let paddr = PhysAddr::from(0x233000);
let pte = X64PTE::new_page(
    paddr,
    /* flags: */ MappingFlags::READ | MappingFlags::WRITE,
    /* is_huge: */ false,
);
assert!(!pte.is_unused());
assert!(pte.is_present());
assert_eq!(pte.paddr(), paddr);
assert_eq!(
    pte.bits(),
    0x800_0000000233_003, // PRESENT | WRITE | NO_EXECUTE | paddr(0x233000)
);
```
