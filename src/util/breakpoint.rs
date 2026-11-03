use anyhow::{ anyhow, Context, Result };
use object::read::elf::ElfFile64;
use object::{ Endianness, Object, ObjectSymbol };

/* A guest address to report when the vcpu is about to execute it, from
 * --break. The address is linear (virtual once the guest pages). Only
 * x86 sets them so far. */
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub struct Breakpoint {
    pub addr: u64,
    /* What it was given as, for the report. */
    pub label: String,
    pub hits: u64
}

/* Hex with 0x, or decimal. */
pub fn parse_number(s: &str) -> Result<u64, String> {
    match s.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => s.parse()
    }.map_err(|e| format!("invalid number {}: {}", s, e))
}

/* ADDRESS, or SYMBOL[+OFFSET] looked up in the ELF symbol table of
 * elf_data. */
pub fn resolve_breakpoint(spec: &str, elf_data: Option<&[u8]>) -> Result<Breakpoint> {
    let invalid = |msg: String| anyhow!(msg);
    let breakpoint = |addr| Ok(Breakpoint { addr, label: spec.to_string(), hits: 0 });

    if spec.starts_with(|c: char| c.is_ascii_digit()) {
        return breakpoint(parse_number(spec).map_err(invalid)?);
    }

    let (name, offset) = match spec.split_once('+') {
        Some((name, offset)) => (name, parse_number(offset).map_err(invalid)?),
        None => (spec, 0)
    };
    let elf_data = elf_data.ok_or(invalid(format!("Breakpoint {}: symbols need an ELF kernel.", spec)))?;
    let elf = ElfFile64::<Endianness>::parse(elf_data)
        .with_context(|| format!("Breakpoint {}: kernel is not an ELF", spec))?;
    /* Defined symbols only: an undefined one has no address. */
    let symbol = elf.symbols()
        .find(|s| s.is_definition() && s.name() == Ok(name))
        .ok_or(invalid(format!("Breakpoint {}: no symbol {} in the kernel.", spec, name)))?;
    breakpoint(symbol.address() + offset)
}
