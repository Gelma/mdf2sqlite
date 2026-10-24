#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::path::Path;

pub const PAGE_SIZE: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PagePointer {
    pub file_id: u16,
    pub page_id: u32,
}

impl PagePointer {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 6 {
            return None;
        }
        let page_id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let file_id = u16::from_le_bytes([data[4], data[5]]);
        if file_id == 0 || (file_id == 1 && page_id == 0) {
            None
        } else {
            Some(Self { file_id, page_id })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotPointer {
    pub page: PagePointer,
    pub slot_id: u16,
}

impl SlotPointer {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        let page = PagePointer::parse(&data[0..6])?;
        let slot_id = u16::from_le_bytes([data[6], data[7]]);
        Some(Self { page, slot_id })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageType {
    Unallocated,
    Data,
    Index,
    TextMix,
    TextTree,
    Sort,
    GAM,
    SGAM,
    IAM,
    PFS,
    Boot,
    FileHeader,
    DiffMap,
    BCM,
    Unknown(u8),
}

impl PageType {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Unallocated,
            1 => Self::Data,
            2 => Self::Index,
            3 => Self::TextMix,
            4 => Self::TextTree,
            7 => Self::Sort,
            8 => Self::GAM,
            9 => Self::SGAM,
            10 => Self::IAM,
            11 => Self::PFS,
            13 => Self::Boot,
            15 => Self::FileHeader,
            16 => Self::DiffMap,
            17 => Self::BCM,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PageHeader {
    pub header_version: u8,
    pub page_type: PageType,
    pub type_flag_bits: u8,
    pub level: u8,
    pub flag_bits: u16,
    pub index_id: u16,
    pub prev_page: Option<PagePointer>,
    pub p_min_len: u16,
    pub next_page: Option<PagePointer>,
    pub slot_count: u16,
    pub object_id: i32,
    pub free_cnt: u16,
    pub free_data: u16,
    pub pointer: Option<PagePointer>,
    pub ghost_rec_cnt: u16,
    pub torn_bits: u32,
}

impl PageHeader {
    pub fn parse(data: &[u8; PAGE_SIZE]) -> Self {
        let header_version = data[0];
        let page_type = PageType::from_u8(data[1]);
        let type_flag_bits = data[2];
        let level = data[3];
        let flag_bits = u16::from_le_bytes([data[4], data[5]]);
        let index_id = u16::from_le_bytes([data[6], data[7]]);
        let prev_page = PagePointer::parse(&data[8..14]);
        let p_min_len = u16::from_le_bytes([data[14], data[15]]);
        let next_page = PagePointer::parse(&data[16..22]);
        let slot_count = u16::from_le_bytes([data[22], data[23]]);
        let object_id = i32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let free_cnt = u16::from_le_bytes([data[28], data[29]]);
        let free_data = u16::from_le_bytes([data[30], data[31]]);
        let pointer = PagePointer::parse(&data[32..38]);
        let ghost_rec_cnt = u16::from_le_bytes([data[58], data[59]]);
        let torn_bits = u32::from_le_bytes([data[60], data[61], data[62], data[63]]);

        Self {
            header_version,
            page_type,
            type_flag_bits,
            level,
            flag_bits,
            index_id,
            prev_page,
            p_min_len,
            next_page,
            slot_count,
            object_id,
            free_cnt,
            free_data,
            pointer,
            ghost_rec_cnt,
            torn_bits,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.header_version == 1 && (self.slot_count as usize) <= (PAGE_SIZE - 96) / 2
    }

    pub fn has_torn_bits(&self) -> bool {
        (self.flag_bits & 0x0100) != 0
    }

    pub fn is_compressed(&self) -> bool {
        (self.flag_bits & 0x0088) != 0
    }
}

#[derive(Clone)]
pub struct PageBuffer {
    pub header: PageHeader,
    pub data: [u8; PAGE_SIZE],
}

impl PageBuffer {
    pub fn from_raw(raw: &[u8]) -> Option<Self> {
        if raw.len() < PAGE_SIZE {
            return None;
        }
        let mut data = [0u8; PAGE_SIZE];
        data.copy_from_slice(&raw[..PAGE_SIZE]);

        let flag_bits = u16::from_le_bytes([data[4], data[5]]);
        if data[0] == 1 && (flag_bits & 0x0100) != 0 {
            let torn_bits = u32::from_le_bytes([data[60], data[61], data[62], data[63]]);
            // Bits 0..1 of torn_bits hold the 2-bit torn signature written to sectors 1..15;
            // Sector 0 (byte 511) is never modified because m_tornBits itself resides in Sector 0.
            for s in 1..16 {
                let sector_end = (s + 1) * 512 - 1;
                let orig_bits = ((torn_bits >> (s * 2)) & 0x03) as u8;
                data[sector_end] = (data[sector_end] & 0xFC) | orig_bits;
            }
        }

        let header = PageHeader::parse(&data);
        if !header.is_valid() {
            return None;
        }

        Some(Self { header, data })
    }

    pub fn slot_offset(&self, slot_idx: u16) -> Option<usize> {
        if slot_idx >= self.header.slot_count {
            return None;
        }
        let pos = PAGE_SIZE.checked_sub(2 * (slot_idx as usize + 1))?;
        let off = u16::from_le_bytes([self.data[pos], self.data[pos + 1]]) as usize;
        if off < 96 || off >= PAGE_SIZE {
            None
        } else {
            Some(off)
        }
    }

    pub fn slot_slice(&self, slot_idx: u16) -> Option<&[u8]> {
        let off = self.slot_offset(slot_idx)?;
        Some(&self.data[off..])
    }
}

#[derive(Debug, Clone)]
pub struct BootPageInfo {
    pub dbi_version: u16,
    pub dbi_create_version: u16,
    pub database_name: String,
    pub db_id: u16,
    pub default_collation_id: i32,
    pub first_sys_indexes: PagePointer,
}

impl BootPageInfo {
    pub fn parse(page: &PageBuffer) -> Result<Self> {
        if page.header.page_type != PageType::Boot {
            bail!(
                "Expected Boot page (type 13) at (1:9), found {:?}",
                page.header.page_type
            );
        }
        let rec = page
            .slot_slice(0)
            .context("Boot page missing slot 0 record")?;
        if rec.len() < 4 + 520 {
            bail!("Boot page record too short: {} bytes", rec.len());
        }
        let fixed = &rec[4..];
        let dbi_version = u16::from_le_bytes([fixed[0], fixed[1]]);
        let dbi_create_version = u16::from_le_bytes([fixed[2], fixed[3]]);

        let name_bytes = &fixed[48..304];
        let u16_words: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&w| w != 0 && w != 0x2020)
            .collect();
        let database_name = String::from_utf16_lossy(&u16_words).trim().to_string();

        let db_id = u16::from_le_bytes([fixed[308], fixed[309]]);
        let default_collation_id =
            i32::from_le_bytes([fixed[388], fixed[389], fixed[390], fixed[391]]);
        let first_sys_indexes = PagePointer::parse(&fixed[512..518])
            .context("Boot page has invalid firstSysIndexes pointer")?;

        Ok(Self {
            dbi_version,
            dbi_create_version,
            database_name,
            db_id,
            default_collation_id,
            first_sys_indexes,
        })
    }
}

pub struct MdfStorage {
    files: HashMap<u16, Mmap>,
}

impl MdfStorage {
    pub fn open<P: AsRef<Path>>(primary_mdf: P) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(false)
            .open(primary_mdf.as_ref())
            .with_context(|| format!("Failed to open MDF file in read-only mode {:?}", primary_mdf.as_ref()))?;
        let mmap = unsafe { Mmap::map(&file) }
            .with_context(|| format!("Failed to mmap MDF file {:?}", primary_mdf.as_ref()))?;

        if mmap.len() < 10 * PAGE_SIZE {
            bail!(
                "MDF file {:?} is too small ({} bytes)",
                primary_mdf.as_ref(),
                mmap.len()
            );
        }

        let mut files = HashMap::new();
        files.insert(1u16, mmap);

        Ok(Self { files })
    }

    pub fn add_secondary_file<P: AsRef<Path>>(&mut self, file_id: u16, path: P) -> Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .write(false)
            .open(path.as_ref())
            .with_context(|| format!("Failed to open secondary NDF file in read-only mode {:?}", path.as_ref()))?;
        let mmap = unsafe { Mmap::map(&file) }?;
        self.files.insert(file_id, mmap);
        Ok(())
    }

    pub fn num_pages(&self, file_id: u16) -> u32 {
        self.files
            .get(&file_id)
            .map(|m| (m.len() / PAGE_SIZE) as u32)
            .unwrap_or(0)
    }

    pub fn get_page(&self, ptr: PagePointer) -> Option<PageBuffer> {
        let mmap = self.files.get(&ptr.file_id)?;
        let start = (ptr.page_id as usize).checked_mul(PAGE_SIZE)?;
        let end = start.checked_add(PAGE_SIZE)?;
        if end > mmap.len() {
            return None;
        }
        PageBuffer::from_raw(&mmap[start..end])
    }

    pub fn is_page_allocated_pfs(&self, ptr: PagePointer) -> bool {
        let pfs_page_id = if ptr.page_id < 8088 {
            1
        } else {
            (ptr.page_id / 8088) * 8088
        };
        let offset_in_pfs = (ptr.page_id % 8088) as usize;
        let pfs_ptr = PagePointer {
            file_id: ptr.file_id,
            page_id: pfs_page_id,
        };
        if let Some(pfs_page) = self.get_page(pfs_ptr) {
            if pfs_page.header.page_type == PageType::PFS {
                let pfs_byte_idx = 100 + offset_in_pfs;
                if pfs_byte_idx < PAGE_SIZE {
                    return (pfs_page.data[pfs_byte_idx] & 0x40) != 0;
                }
            }
        }
        true
    }

    /// Descends a B-Tree from `pg_root` via slot 0 down-page pointers until reaching a Leaf Data page (type 1).
    pub fn find_btree_first_leaf(
        &self,
        pg_first: Option<PagePointer>,
        pg_root: Option<PagePointer>,
    ) -> Option<PagePointer> {
        if let Some(mut cur) = pg_root {
            for _ in 0..16 {
                let Some(page) = self.get_page(cur) else {
                    break;
                };
                match page.header.page_type {
                    PageType::Data => return Some(cur),
                    PageType::Index => {
                        let Some(rec0) = page.slot_slice(0) else {
                            break;
                        };
                        let pmin = page.header.p_min_len as usize;
                        if pmin < 7 || rec0.len() < pmin {
                            break;
                        }
                        if let Some(down_ptr) = PagePointer::parse(&rec0[pmin - 6..pmin]) {
                            cur = down_ptr;
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        }
        if let Some(first) = pg_first {
            if let Some(page) = self.get_page(first) {
                if page.header.page_type == PageType::Data {
                    return Some(first);
                }
            }
        }
        None
    }

    /// Follows the `m_nextPage` linked list starting from `start_ptr`.
    pub fn collect_linked_data_pages(&self, start_ptr: PagePointer) -> Vec<PagePointer> {
        let mut pages = Vec::new();
        let mut visited = HashSet::new();
        let mut cur_opt = Some(start_ptr);

        while let Some(cur) = cur_opt {
            if !visited.insert(cur) {
                break;
            }
            let Some(page) = self.get_page(cur) else {
                break;
            };
            if page.header.page_type != PageType::Data {
                break;
            }
            pages.push(cur);
            cur_opt = page.header.next_page;
        }
        pages
    }

    /// Collects all allocated Data Pages (type 1, level 0) tracked by an IAM page chain.
    pub fn collect_iam_data_pages(&self, first_iam: PagePointer) -> Vec<PagePointer> {
        let mut pages = Vec::new();
        let mut visited_iam = HashSet::new();
        let mut visited_data = HashSet::new();
        let mut cur_iam_opt = Some(first_iam);

        while let Some(iam_ptr) = cur_iam_opt {
            if !visited_iam.insert(iam_ptr) {
                break;
            }
            if !self.is_page_allocated_pfs(iam_ptr) {
                break;
            }
            let Some(iam_page) = self.get_page(iam_ptr) else {
                break;
            };
            if iam_page.header.page_type != PageType::IAM {
                break;
            }

            // Slot 0: IAM Header (90 bytes fixed data)
            let mut start_page = PagePointer {
                file_id: iam_ptr.file_id,
                page_id: (iam_ptr.page_id / 511232) * 511232,
            };

            if let Some(rec0) = iam_page.slot_slice(0) {
                if rec0.len() >= 4 + 90 {
                    let hdr = &rec0[4..4 + 90];
                    if let Some(sp) = PagePointer::parse(&hdr[36..42]) {
                        start_page = sp;
                    } else {
                        let sp_id = u32::from_le_bytes([hdr[36], hdr[37], hdr[38], hdr[39]]);
                        let sf_id = u16::from_le_bytes([hdr[40], hdr[41]]);
                        if sf_id != 0 {
                            start_page = PagePointer {
                                file_id: sf_id,
                                page_id: sp_id,
                            };
                        }
                    }

                    // 8 single-page allocations at offsets 42..90
                    for i in 0..8 {
                        let off = 42 + i * 6;
                        if let Some(slot_ptr) = PagePointer::parse(&hdr[off..off + 6]) {
                            if self.is_page_allocated_pfs(slot_ptr) {
                                if let Some(dp) = self.get_page(slot_ptr) {
                                    if dp.header.page_type == PageType::Data && dp.header.level == 0
                                    {
                                        if visited_data.insert(slot_ptr) {
                                            pages.push(slot_ptr);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Slot 1: Extent allocation bitmap (~7988 bytes)
            if let Some(rec1) = iam_page.slot_slice(1) {
                if rec1.len() >= 4 {
                    let fixed_len = u16::from_le_bytes([rec1[2], rec1[3]]) as usize;
                    if fixed_len >= 4 && rec1.len() >= fixed_len {
                        let bitmap = &rec1[4..fixed_len];
                        for (byte_idx, &b) in bitmap.iter().enumerate() {
                            if b == 0 {
                                continue;
                            }
                            for bit in 0..8 {
                                if (b & (1 << bit)) != 0 {
                                    let extent_idx = (byte_idx * 8 + bit) as u32;
                                    let extent_start_page = start_page.page_id + extent_idx * 8;
                                    for offset_in_ext in 0..8 {
                                        let page_ptr = PagePointer {
                                            file_id: start_page.file_id,
                                            page_id: extent_start_page + offset_in_ext,
                                        };
                                        if self.is_page_allocated_pfs(page_ptr) {
                                            if let Some(dp) = self.get_page(page_ptr) {
                                                if dp.header.page_type == PageType::Data
                                                    && dp.header.level == 0
                                                {
                                                    if visited_data.insert(page_ptr) {
                                                        pages.push(page_ptr);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            cur_iam_opt = iam_page.header.next_page;
        }

        pages
    }

    /// Collects all data pages for a partition (handles both Clustered Index B-Trees and Heap IAMs).
    pub fn collect_partition_data_pages(
        &self,
        pg_first: Option<PagePointer>,
        pg_root: Option<PagePointer>,
        pg_first_iam: Option<PagePointer>,
        prefer_linked: bool,
    ) -> Vec<PagePointer> {
        if prefer_linked {
            if let Some(leaf_start) = self.find_btree_first_leaf(pg_first, pg_root) {
                let linked = self.collect_linked_data_pages(leaf_start);
                if !linked.is_empty() {
                    return linked;
                }
            }
        }

        if let Some(iam) = pg_first_iam {
            let iam_pages = self.collect_iam_data_pages(iam);
            if !iam_pages.is_empty() {
                return iam_pages;
            }
        }

        if let Some(leaf_start) = self.find_btree_first_leaf(pg_first, pg_root) {
            return self.collect_linked_data_pages(leaf_start);
        }

        Vec::new()
    }
}
