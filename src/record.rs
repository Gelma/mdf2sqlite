#![allow(dead_code)]

use crate::page::{MdfStorage, PageBuffer, SlotPointer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordType {
    Primary,
    Forwarded,
    ForwardingStub,
    Index,
    BlobFragment,
    GhostIndex,
    GhostData,
    GhostVersion,
}

impl RecordType {
    pub fn from_status_a(status_a: u8) -> Self {
        match (status_a >> 1) & 0x07 {
            0 => Self::Primary,
            1 => Self::Forwarded,
            2 => Self::ForwardingStub,
            3 => Self::Index,
            4 => Self::BlobFragment,
            5 => Self::GhostIndex,
            6 => Self::GhostData,
            7 => Self::GhostVersion,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VarColEntry<'a> {
    pub data: &'a [u8],
    pub is_complex: bool,
}

#[derive(Clone, Debug)]
pub struct SparseVector<'a> {
    raw: &'a [u8],
    col_count: usize,
    is_complex: bool,
}

impl<'a> SparseVector<'a> {
    pub fn parse(raw: &'a [u8]) -> Option<Self> {
        if raw.len() < 4 {
            return None;
        }
        let complex_header = u16::from_le_bytes([raw[0], raw[1]]);
        if complex_header != 5 && complex_header != 261 {
            return None;
        }
        let is_complex = complex_header == 261;
        let col_count = u16::from_le_bytes([raw[2], raw[3]]) as usize;
        let header_end = 4 + col_count * 4;
        if raw.len() < header_end {
            return None;
        }
        Some(Self {
            raw,
            col_count,
            is_complex,
        })
    }

    pub fn get_column(&self, column_id: u16) -> Option<VarColEntry<'a>> {
        // Binary search or linear scan over col_count (usually small)
        for i in 0..self.col_count {
            let id_pos = 4 + i * 2;
            let cid = u16::from_le_bytes([self.raw[id_pos], self.raw[id_pos + 1]]);
            if cid == column_id {
                let off_table_base = 4 + self.col_count * 2;
                let start = if i == 0 {
                    4 + self.col_count * 4
                } else {
                    let prev_pos = off_table_base + (i - 1) * 2;
                    let prev_raw = u16::from_le_bytes([self.raw[prev_pos], self.raw[prev_pos + 1]]);
                    (prev_raw & 0x7FFF) as usize
                };
                let cur_pos = off_table_base + i * 2;
                let cur_raw = u16::from_le_bytes([self.raw[cur_pos], self.raw[cur_pos + 1]]);
                let end = (cur_raw & 0x7FFF) as usize;
                if start <= end && end <= self.raw.len() {
                    return Some(VarColEntry {
                        data: &self.raw[start..end],
                        is_complex: self.is_complex || (cur_raw & 0x8000) != 0,
                    });
                }
                return None;
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct ParsedRecord<'a> {
    pub raw: &'a [u8],
    pub status_a: u8,
    pub status_b: u8,
    pub record_type: RecordType,
    pub fixed_len: usize,
    null_bitmap: &'a [u8],
    pub num_cols_in_bitmap: usize,
    var_entries: Vec<VarColEntry<'a>>,
    pub sparse_vector: Option<SparseVector<'a>>,
}

impl<'a> ParsedRecord<'a> {
    pub fn parse(raw: &'a [u8]) -> Option<Self> {
        if raw.len() < 4 {
            return None;
        }
        let status_a = raw[0];
        let status_b = raw[1];
        let record_type = RecordType::from_status_a(status_a);
        if record_type != RecordType::Primary && record_type != RecordType::Forwarded {
            return None;
        }

        let has_null_bitmap = (status_a & 0x10) != 0;
        let has_var_cols = (status_a & 0x20) != 0;

        let fixed_len = u16::from_le_bytes([raw[2], raw[3]]) as usize;
        if fixed_len < 4 || fixed_len > raw.len() {
            return None;
        }

        let mut cursor = fixed_len;
        let mut num_cols_in_bitmap = 0;
        let mut null_bitmap: &[u8] = &[];

        if (has_null_bitmap || has_var_cols) && cursor + 2 <= raw.len() {
            num_cols_in_bitmap = u16::from_le_bytes([raw[cursor], raw[cursor + 1]]) as usize;
            cursor += 2;
        }

        if has_null_bitmap {
            let bm_bytes = (num_cols_in_bitmap + 7) / 8;
            let bm_end = (cursor + bm_bytes).min(raw.len());
            null_bitmap = &raw[cursor..bm_end];
            cursor = bm_end;
        }

        let mut var_entries = Vec::new();
        let mut sparse_vector = None;

        if has_var_cols {
            let num_var_cols = if fixed_len == 4 && !has_null_bitmap {
                num_cols_in_bitmap
            } else if cursor + 2 <= raw.len() {
                let n = u16::from_le_bytes([raw[cursor], raw[cursor + 1]]) as usize;
                cursor += 2;
                n
            } else {
                0
            };
            let offsets_start = cursor;
            let data_start = offsets_start + num_var_cols * 2;
            if data_start <= raw.len() {
                var_entries.reserve(num_var_cols);
                let mut cur_start = data_start;
                for i in 0..num_var_cols {
                    let off_pos = offsets_start + i * 2;
                    let raw_off = u16::from_le_bytes([raw[off_pos], raw[off_pos + 1]]);
                    let is_complex = (raw_off & 0x8000) != 0;
                    let end_off = (raw_off & 0x7FFF) as usize;
                    let slice = if end_off >= cur_start && end_off <= raw.len() {
                        let s = &raw[cur_start..end_off];
                        cur_start = end_off;
                        s
                    } else {
                        &[]
                    };
                    var_entries.push(VarColEntry {
                        data: slice,
                        is_complex,
                    });
                }

                // Check if the last variable-length entry is an inline Sparse Vector (complex_id == 5 or 261)
                if let Some(last) = var_entries.last() {
                    if last.is_complex && last.data.len() >= 4 {
                        let cid = u16::from_le_bytes([last.data[0], last.data[1]]);
                        if cid == 5 || cid == 261 {
                            sparse_vector = SparseVector::parse(last.data);
                        }
                    }
                }
            }
        }

        Some(Self {
            raw,
            status_a,
            status_b,
            record_type,
            fixed_len,
            null_bitmap,
            num_cols_in_bitmap,
            var_entries,
            sparse_vector,
        })
    }

    /// Checks whether a 1-based `leaf_nullbit` is marked NULL in the record's null bitmap.
    pub fn is_null(&self, leaf_nullbit: i16) -> bool {
        if leaf_nullbit <= 0 {
            return false;
        }
        let bit_idx = (leaf_nullbit - 1) as usize;
        let byte_idx = bit_idx / 8;
        let bit_in_byte = bit_idx % 8;
        if byte_idx < self.null_bitmap.len() {
            (self.null_bitmap[byte_idx] & (1 << bit_in_byte)) != 0
        } else {
            // If column was added via ALTER TABLE ADD COLUMN after this row was written,
            // its null bit is beyond `num_cols_in_bitmap` and defaults to NULL.
            bit_idx >= self.num_cols_in_bitmap && self.num_cols_in_bitmap > 0
        }
    }

    /// Reads fixed-length bytes at 0-based offset from start of record (`leaf_offset >= 4`).
    pub fn fixed_slice(&self, leaf_offset: usize, len: usize) -> Option<&'a [u8]> {
        let end = leaf_offset.checked_add(len)?;
        if end <= self.fixed_len && end <= self.raw.len() {
            Some(&self.raw[leaf_offset..end])
        } else {
            None
        }
    }

    /// Reads a variable-length column entry by 0-based index (`(-leaf_offset) - 1`).
    pub fn var_entry(&self, var_idx: usize) -> Option<VarColEntry<'a>> {
        self.var_entries.get(var_idx).cloned()
    }
}

/// Resolves a slot on a Data Page into its raw record bytes, following a Forwarding Stub if present.
/// Returns `None` if the slot is a Ghost record, an unresolvable stub, or not a Primary/Forwarded record.
/// When reading pages sequentially from an IAM or linked list, remember that both the ForwardingStub
/// AND the ForwardedRecord sit on data pages of the table!
///Wait: if we iterate over all Data Pages of a Heap, every allocated Data Page has slots.
/// A forwarded record appears ONCE as a `ForwardingStub` (type 2) on its original page, and ONCE as a
/// `Forwarded` record (type 1) on its target page!
/// Therefore, if we process all Data Pages of a table, accepting BOTH `Primary` (type 0) and `Forwarded` (type 1)
/// directly on the page where they reside (and ignoring `ForwardingStub` type 2) visits every row
/// EXACTLY ONCE without even needing an extra page lookup!
/// However, for system catalog lookups or when following a stub explicitly, `resolve_slot_record` can follow it.
pub fn resolve_slot_record<'a>(
    storage: &'a MdfStorage,
    page: &'a PageBuffer,
    slot_idx: u16,
    owned_page_slot: &'a mut Option<PageBuffer>,
) -> Option<ParsedRecord<'a>> {
    let raw = page.slot_slice(slot_idx)?;
    if raw.is_empty() {
        return None;
    }
    let rec_type = RecordType::from_status_a(raw[0]);
    match rec_type {
        RecordType::Primary | RecordType::Forwarded => ParsedRecord::parse(raw),
        RecordType::ForwardingStub => {
            if raw.len() < 9 {
                return None;
            }
            let target = SlotPointer::parse(&raw[1..9])?;
            *owned_page_slot = storage.get_page(target.page);
            let target_page = owned_page_slot.as_ref()?;
            let target_raw = target_page.slot_slice(target.slot_id)?;
            ParsedRecord::parse(target_raw)
        }
        _ => None,
    }
}
