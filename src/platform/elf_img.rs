// 解析磁盘上的 libart.so，给 LSPlant 提供 ART 符号解析
// 内存解析在 Android 14+ 上不可靠（relro 段会被 mprotect 为 ---p），统一走文件路径

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::gnu_debugdata;
use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;

const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;
const SHT_DYNSYM: u32 = 11;
const SHN_UNDEF: u16 = 0;
const SHN_LORESERVE: u16 = 0xff00;
const SHN_HIRESERVE: u16 = 0xffff;
const R_AARCH64_GLOB_DAT: u32 = 1025;
const R_AARCH64_JUMP_SLOT: u32 = 1026;

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Shdr {
    sh_name: u32,
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Sym {
    st_name: u32,
    st_info: u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Rel {
    r_offset: u64,
    r_info: u64,
}

pub struct ElfImg {
    load_bias: usize,
    image: Vec<u8>,
}

struct EmbeddedElf {
    apk_path: String,
    entry_name: String,
    data_offset: usize,
    data_size: usize,
    image: Vec<u8>,
}

impl ElfImg {
    // 按路径后缀匹配首个已加载模块；未命中或文件不可读返回 None
    pub fn load(path_suffix: &str) -> Option<Self> {
        let (load_bias, image) =
            if let Some((load_bias, file_path)) = find_module_load_bias(path_suffix) {
                let image = match std::fs::read(&file_path) {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("elf image read failed path={} err={}", file_path, e);
                        return None;
                    }
                };
                (load_bias, image)
            } else {
                let embedded = find_embedded_elf(path_suffix)?;
                let load_bias = find_embedded_elf_load_bias(
                    &embedded.apk_path,
                    embedded.data_offset,
                    embedded.data_size,
                )?;
                log::info!(
                    "elf image loaded from apk entry path={} entry={} off={} size={}",
                    embedded.apk_path,
                    embedded.entry_name,
                    embedded.data_offset,
                    embedded.data_size
                );
                (load_bias, embedded.image)
            };
        if image.len() < size_of::<Elf64Ehdr>() {
            return None;
        }
        if image[..4] != [0x7f, b'E', b'L', b'F'] {
            return None;
        }
        Some(ElfImg { load_bias, image })
    }

    pub fn base(&self) -> usize {
        self.load_bias
    }

    // 精确匹配符号名；命中返回运行时函数地址，未命中返回 null
    pub fn find(&self, name: &str) -> *mut c_void {
        let bytes = name.as_bytes();
        self.find_by(|n| n == bytes)
    }

    // 前缀匹配；用于 LSPlant 的 PrefixResolver（mangled 名含参数包变体）
    pub fn find_prefix(&self, prefix: &str) -> *mut c_void {
        let bytes = prefix.as_bytes();
        self.find_by(|n| n.starts_with(bytes))
    }

    pub fn find_plt_slots(&self, name: &str) -> Vec<usize> {
        let bytes = name.as_bytes();
        self.find_relocation_slots_by(|n| n == bytes)
    }

    fn find_by<F: Fn(&[u8]) -> bool>(&self, matches: F) -> *mut c_void {
        let Some(ehdr) = parse_file_ehdr(&self.image) else {
            return std::ptr::null_mut();
        };
        if let Some(addr) = lookup_dynsym(&self.image, &ehdr, self.load_bias, &matches) {
            return addr as *mut c_void;
        }
        if let Some(addr) = lookup_image_symtab(&self.image, &ehdr, self.load_bias, &matches) {
            return addr as *mut c_void;
        }
        if let Some(addr) = lookup_debugdata_symtab(&self.image, &ehdr, self.load_bias, &matches) {
            return addr as *mut c_void;
        }
        std::ptr::null_mut()
    }

    fn find_relocation_slots_by<F: Fn(&[u8]) -> bool>(&self, matches: F) -> Vec<usize> {
        let Some(ehdr) = parse_file_ehdr(&self.image) else {
            return Vec::new();
        };
        let mut slots = Vec::new();
        for i in 0..ehdr.e_shnum as usize {
            let Some(reloc) = section_header(&self.image, &ehdr, i) else {
                continue;
            };
            let is_rela =
                reloc.sh_type == SHT_RELA && reloc.sh_entsize as usize == size_of::<Elf64Rela>();
            let is_rel =
                reloc.sh_type == SHT_REL && reloc.sh_entsize as usize == size_of::<Elf64Rel>();
            if !is_rela && !is_rel {
                continue;
            }
            if reloc.sh_link >= ehdr.e_shnum as u32 {
                continue;
            }
            let Some(symtab) = section_header(&self.image, &ehdr, reloc.sh_link as usize) else {
                continue;
            };
            if symtab.sh_entsize as usize != size_of::<Elf64Sym>()
                || symtab.sh_link >= ehdr.e_shnum as u32
            {
                continue;
            }
            let Some(strtab_header) = section_header(&self.image, &ehdr, symtab.sh_link as usize)
            else {
                continue;
            };
            if strtab_header.sh_type != SHT_STRTAB {
                continue;
            }
            let Some(strtab) = section_bytes_by_header(&self.image, &strtab_header) else {
                continue;
            };
            let count = (reloc.sh_size / reloc.sh_entsize) as usize;
            for idx in 0..count {
                let off = reloc.sh_offset as usize + idx * reloc.sh_entsize as usize;
                let (r_offset, r_info) = if is_rela {
                    let Some(rela) = read_struct::<Elf64Rela>(&self.image, off) else {
                        continue;
                    };
                    (rela.r_offset, rela.r_info)
                } else {
                    let Some(rel) = read_struct::<Elf64Rel>(&self.image, off) else {
                        continue;
                    };
                    (rel.r_offset, rel.r_info)
                };
                let rel_type = (r_info & 0xffff_ffff) as u32;
                if rel_type != R_AARCH64_JUMP_SLOT && rel_type != R_AARCH64_GLOB_DAT {
                    continue;
                }
                let sym_idx = (r_info >> 32) as usize;
                let sym_off = symtab.sh_offset as usize + sym_idx * size_of::<Elf64Sym>();
                let Some(sym) = read_struct::<Elf64Sym>(&self.image, sym_off) else {
                    continue;
                };
                let Some(sym_name) = read_c_bytes(strtab, sym.st_name as usize) else {
                    continue;
                };
                if matches(sym_name) {
                    slots.push(self.load_bias + r_offset as usize);
                }
            }
        }
        slots.sort_unstable();
        slots.dedup();
        slots
    }
}

// /proc/self/maps：load_bias = mapping_addr - mapping_offset（典型 .so 首段 p_vaddr==p_offset==0）
// 即便 ELF 头那段被 relro 保护掉，其它段也能算出同一个 bias
fn find_module_load_bias(suffix: &str) -> Option<(usize, String)> {
    let content = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let addr_range = parts.next()?;
        let _perms = parts.next()?;
        let offset_str = parts.next()?;
        let _dev = parts.next()?;
        let _inode = parts.next()?;
        let Some(path) = parts.next() else {
            continue;
        };
        if !path.ends_with(suffix) {
            continue;
        }
        let dash = addr_range.find('-')?;
        let base = usize::from_str_radix(&addr_range[..dash], 16).ok()?;
        let offset = usize::from_str_radix(offset_str, 16).ok()?;
        let load_bias = base.checked_sub(offset)?;
        return Some((load_bias, resolve_map_path(path)));
    }
    None
}

fn find_embedded_elf(entry_suffix: &str) -> Option<EmbeddedElf> {
    let content = std::fs::read_to_string("/proc/self/maps").ok()?;
    let mut apk_paths = Vec::new();
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(_addr_range) = parts.next() else {
            continue;
        };
        let Some(_perms) = parts.next() else {
            continue;
        };
        let Some(_offset_str) = parts.next() else {
            continue;
        };
        let Some(_dev) = parts.next() else {
            continue;
        };
        let Some(_inode) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else {
            continue;
        };
        if !path.ends_with(".apk") || apk_paths.iter().any(|known| known == path) {
            continue;
        }
        apk_paths.push(path.to_string());
    }

    for apk_path in apk_paths {
        let file_path = resolve_map_path(&apk_path);
        let Ok(file) = std::fs::read(&file_path) else {
            continue;
        };
        if let Some(embedded) = find_zip_stored_elf(&apk_path, &file, entry_suffix) {
            return Some(embedded);
        }
    }
    None
}

fn find_embedded_elf_load_bias(
    apk_path: &str,
    data_offset: usize,
    data_size: usize,
) -> Option<usize> {
    let content = std::fs::read_to_string("/proc/self/maps").ok()?;
    let data_end = data_offset.checked_add(data_size)?;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(addr_range) = parts.next() else {
            continue;
        };
        let Some(perms) = parts.next() else {
            continue;
        };
        let Some(offset_str) = parts.next() else {
            continue;
        };
        let Some(_dev) = parts.next() else {
            continue;
        };
        let Some(_inode) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else {
            continue;
        };
        if path != apk_path || !perms.contains('x') {
            continue;
        }
        let Ok(offset) = usize::from_str_radix(offset_str, 16) else {
            continue;
        };
        if offset < data_offset || offset >= data_end {
            continue;
        }
        let Some(dash) = addr_range.find('-') else {
            continue;
        };
        let Ok(base) = usize::from_str_radix(&addr_range[..dash], 16) else {
            continue;
        };
        let Some(offset_delta) = offset.checked_sub(data_offset) else {
            continue;
        };
        if let Some(load_bias) = base.checked_sub(offset_delta) {
            return Some(load_bias);
        }
    }
    None
}

fn find_zip_stored_elf(apk_path: &str, file: &[u8], entry_suffix: &str) -> Option<EmbeddedElf> {
    const EOCD_SIG: u32 = 0x0605_4b50;
    const CD_SIG: u32 = 0x0201_4b50;
    const LFH_SIG: u32 = 0x0403_4b50;
    const METHOD_STORED: u16 = 0;

    let eocd = find_eocd(file)?;
    if read_u32(file, eocd)? != EOCD_SIG {
        return None;
    }
    let cd_count = read_u16(file, eocd + 10)? as usize;
    let mut cd_offset = read_u32(file, eocd + 16)? as usize;

    for _ in 0..cd_count {
        if read_u32(file, cd_offset)? != CD_SIG {
            return None;
        }
        let method = read_u16(file, cd_offset + 10)?;
        let compressed_size = read_u32(file, cd_offset + 20)? as usize;
        let uncompressed_size = read_u32(file, cd_offset + 24)? as usize;
        let name_len = read_u16(file, cd_offset + 28)? as usize;
        let extra_len = read_u16(file, cd_offset + 30)? as usize;
        let comment_len = read_u16(file, cd_offset + 32)? as usize;
        let local_header_offset = read_u32(file, cd_offset + 42)? as usize;
        let name_start = cd_offset + 46;
        let name_end = name_start.checked_add(name_len)?;
        let name = std::str::from_utf8(file.get(name_start..name_end)?).ok()?;

        if name.ends_with(entry_suffix) && method == METHOD_STORED {
            if read_u32(file, local_header_offset)? != LFH_SIG {
                return None;
            }
            let local_name_len = read_u16(file, local_header_offset + 26)? as usize;
            let local_extra_len = read_u16(file, local_header_offset + 28)? as usize;
            let data_offset = local_header_offset
                .checked_add(30)?
                .checked_add(local_name_len)?
                .checked_add(local_extra_len)?;
            let data_size = uncompressed_size.min(compressed_size);
            let data_end = data_offset.checked_add(data_size)?;
            let image = file.get(data_offset..data_end)?.to_vec();
            if image.len() >= 4 && image[..4] == [0x7f, b'E', b'L', b'F'] {
                return Some(EmbeddedElf {
                    apk_path: apk_path.to_string(),
                    entry_name: name.to_string(),
                    data_offset,
                    data_size,
                    image,
                });
            }
        }

        cd_offset = name_end.checked_add(extra_len)?.checked_add(comment_len)?;
    }

    None
}

fn find_eocd(file: &[u8]) -> Option<usize> {
    const EOCD_SIG_BYTES: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    let min = file.len().saturating_sub(66_000);
    let max = file.len().checked_sub(4)?;
    (min..=max)
        .rev()
        .find(|&offset| file.get(offset..offset + 4) == Some(&EOCD_SIG_BYTES))
}

fn read_u16(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = buf.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn resolve_map_path(path: &str) -> String {
    if !path.starts_with("/apex/") || path.contains("@") {
        return path.to_string();
    }
    let candidate = PathBuf::from(path);
    if candidate.exists() {
        return path.to_string();
    }
    path.replacen("/apex/", "/system/apex/", 1)
}

fn lookup_dynsym<F: Fn(&[u8]) -> bool>(
    image: &[u8],
    ehdr: &Elf64Ehdr,
    load_bias: usize,
    matches: &F,
) -> Option<usize> {
    for i in 0..ehdr.e_shnum as usize {
        let dynsym = section_header(image, ehdr, i)?;
        if dynsym.sh_type != SHT_DYNSYM || dynsym.sh_entsize as usize != size_of::<Elf64Sym>() {
            continue;
        }
        if dynsym.sh_link >= ehdr.e_shnum as u32 {
            continue;
        }
        let dynstr = section_header(image, ehdr, dynsym.sh_link as usize)?;
        if dynstr.sh_type != SHT_STRTAB {
            continue;
        }
        let strtab = section_bytes_by_header(image, &dynstr)?;
        let count = (dynsym.sh_size / dynsym.sh_entsize) as usize;
        for idx in 0..count {
            let off = dynsym.sh_offset as usize + idx * size_of::<Elf64Sym>();
            let sym = read_struct::<Elf64Sym>(image, off)?;
            if sym.st_name == 0 || sym.st_value == 0 || !is_export_symtab_symbol(sym.st_shndx) {
                continue;
            }
            let Some(name) = read_c_bytes(strtab, sym.st_name as usize) else {
                continue;
            };
            if matches(name) {
                return Some(load_bias + sym.st_value as usize);
            }
        }
    }
    None
}

fn lookup_image_symtab<F: Fn(&[u8]) -> bool>(
    image: &[u8],
    ehdr: &Elf64Ehdr,
    load_bias: usize,
    matches: &F,
) -> Option<usize> {
    let shstr = section_bytes(image, ehdr, ehdr.e_shstrndx as usize)?;
    for i in 0..ehdr.e_shnum as usize {
        let symtab = section_header(image, ehdr, i)?;
        if symtab.sh_type != SHT_SYMTAB || symtab.sh_entsize as usize != size_of::<Elf64Sym>() {
            continue;
        }
        if !section_name_is(shstr, symtab.sh_name, b".symtab") {
            continue;
        }
        if let Some(found) = lookup_symtab_section(image, ehdr, &symtab, load_bias, matches) {
            return Some(found);
        }
    }
    None
}

fn lookup_debugdata_symtab<F: Fn(&[u8]) -> bool>(
    image: &[u8],
    ehdr: &Elf64Ehdr,
    load_bias: usize,
    matches: &F,
) -> Option<usize> {
    let shstr = section_bytes(image, ehdr, ehdr.e_shstrndx as usize)?;
    for i in 0..ehdr.e_shnum as usize {
        let debugdata = section_header(image, ehdr, i)?;
        if debugdata.sh_type != SHT_PROGBITS
            || !section_name_is(shstr, debugdata.sh_name, b".gnu_debugdata")
        {
            continue;
        }
        let zipped = section_bytes_by_header(image, &debugdata)?;
        let unpacked = gnu_debugdata::decompress(zipped)?;
        let debug_ehdr = parse_file_ehdr(&unpacked)?;
        if let Some(found) = lookup_image_symtab(&unpacked, &debug_ehdr, load_bias, matches) {
            return Some(found);
        }
    }
    None
}

fn lookup_symtab_section<F: Fn(&[u8]) -> bool>(
    image: &[u8],
    ehdr: &Elf64Ehdr,
    symtab: &Elf64Shdr,
    load_bias: usize,
    matches: &F,
) -> Option<usize> {
    if symtab.sh_link >= ehdr.e_shnum as u32 {
        return None;
    }
    let strtab = section_bytes(image, ehdr, symtab.sh_link as usize)?;
    let count = (symtab.sh_size / symtab.sh_entsize) as usize;
    for idx in 0..count {
        let off = symtab.sh_offset as usize + idx * size_of::<Elf64Sym>();
        let sym = read_struct::<Elf64Sym>(image, off)?;
        if sym.st_name == 0 || sym.st_value == 0 || !is_export_symtab_symbol(sym.st_shndx) {
            continue;
        }
        let name = read_c_bytes(strtab, sym.st_name as usize)?;
        if matches(name) {
            return Some(load_bias + sym.st_value as usize);
        }
    }
    None
}

fn parse_file_ehdr(image: &[u8]) -> Option<Elf64Ehdr> {
    let ehdr = read_struct::<Elf64Ehdr>(image, 0)?;
    if ehdr.e_ident[..4] != [0x7f, b'E', b'L', b'F'] {
        return None;
    }
    Some(ehdr)
}

fn section_header(image: &[u8], ehdr: &Elf64Ehdr, idx: usize) -> Option<Elf64Shdr> {
    if idx >= ehdr.e_shnum as usize {
        return None;
    }
    let offset = ehdr.e_shoff as usize + idx * ehdr.e_shentsize as usize;
    read_struct::<Elf64Shdr>(image, offset)
}

fn section_bytes<'a>(image: &'a [u8], ehdr: &Elf64Ehdr, idx: usize) -> Option<&'a [u8]> {
    let sh = section_header(image, ehdr, idx)?;
    section_bytes_by_header(image, &sh)
}

fn section_bytes_by_header<'a>(image: &'a [u8], sh: &Elf64Shdr) -> Option<&'a [u8]> {
    let start = sh.sh_offset as usize;
    let end = start.checked_add(sh.sh_size as usize)?;
    image.get(start..end)
}

fn section_name_is(shstr: &[u8], name_off: u32, expected: &[u8]) -> bool {
    matches!(read_c_bytes(shstr, name_off as usize), Some(name) if name == expected)
}

fn read_struct<T: Copy>(image: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(size_of::<T>())?;
    if end > image.len() {
        return None;
    }
    let ptr = image[offset..end].as_ptr() as *const T;
    Some(unsafe { std::ptr::read_unaligned(ptr) })
}

fn read_c_bytes(buf: &[u8], offset: usize) -> Option<&[u8]> {
    if offset >= buf.len() {
        return None;
    }
    let tail = &buf[offset..];
    let end = tail.iter().position(|&b| b == 0)?;
    Some(&tail[..end])
}

fn is_export_symtab_symbol(shndx: u16) -> bool {
    shndx != SHN_UNDEF && !(SHN_LORESERVE..=SHN_HIRESERVE).contains(&shndx)
}
