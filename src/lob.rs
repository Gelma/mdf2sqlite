use crate::page::{MdfStorage, PagePointer, SlotPointer};
use crate::record::VarColEntry;

/// Resolves a variable-length column entry (`VarColEntry`), expanding off-row LOB pointers
/// (`TextPointer` 16B or `BlobInlineRoot` / Row-Overflow `complex_id == 2 | 4`) if `is_complex` is true.
pub fn resolve_var_bytes(storage: &MdfStorage, entry: &VarColEntry<'_>) -> Vec<u8> {
    if !entry.is_complex {
        return entry.data.to_vec();
    }

    let raw = entry.data;
    if raw.is_empty() {
        return Vec::new();
    }

    // 1. 16-byte legacy TextPointer (TEXT, NTEXT, IMAGE)
    // Bytes 0..4: timestamp, 4..8: reserved, 8..16: SlotPointer (PageID 4B, FileID 2B, SlotID 2B)
    if raw.len() == 16 && raw[0] != 2 && raw[0] != 4 && raw[0] != 5 {
        if let Some(slot_ptr) = SlotPointer::parse(&raw[8..16]) {
            let mut out = Vec::new();
            read_lob_slot_recursive(storage, slot_ptr, 0, &mut out);
            return out;
        }
    }

    let complex_type = raw[0];
    match complex_type {
        // 2 = Row-Overflow pointer, 4 = BLOB Inline Root
        2 | 4 => {
            if raw.len() < 12 {
                return Vec::new();
            }
            // Note: in SQL Server 2005+, if complex_type == 4 and raw[1] == 0 (indexLevel == 0)
            // wait: in BlobInlineRootProxy (SQL Server 2008 Internals Table 7-1):
            // Bytes 0: complexColumnType (2 or 4)
            // Bytes 1..3: indexLevel (i16)
            // Byte 3: unused
            // Bytes 4..8: sequence (i32)
            // Bytes 8..12: timestamp (u32)
            // Bytes 12..: 12-byte LobSlotPointers: (length: i32, page_id: u32, file_id: u16, slot_id: u16)
            let mut out = Vec::new();
            let mut offset = 12;
            while offset + 12 <= raw.len() {
                let page_id = u32::from_le_bytes([
                    raw[offset + 4],
                    raw[offset + 5],
                    raw[offset + 6],
                    raw[offset + 7],
                ]);
                let file_id = u16::from_le_bytes([raw[offset + 8], raw[offset + 9]]);
                let slot_id = u16::from_le_bytes([raw[offset + 10], raw[offset + 11]]);
                if file_id != 0 {
                    let slot_ptr = SlotPointer {
                        page: PagePointer { file_id, page_id },
                        slot_id,
                    };
                    read_lob_slot_recursive(storage, slot_ptr, 0, &mut out);
                }
                offset += 12;
            }
            out
        }
        // If a 16-byte TextPointer happened to start with timestamp byte 2, 4, or 5
        _ if raw.len() == 16 => {
            if let Some(slot_ptr) = SlotPointer::parse(&raw[8..16]) {
                let mut out = Vec::new();
                read_lob_slot_recursive(storage, slot_ptr, 0, &mut out);
                return out;
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn read_lob_slot_recursive(
    storage: &MdfStorage,
    slot_ptr: SlotPointer,
    depth: usize,
    out: &mut Vec<u8>,
) {
    if depth > 12 {
        return;
    }
    let Some(page) = storage.get_page(slot_ptr.page) else {
        return;
    };
    let Some(rec) = page.slot_slice(slot_ptr.slot_id) else {
        return;
    };
    if rec.len() < 14 {
        return;
    }
    let fixed_len = (u16::from_le_bytes([rec[2], rec[3]]) as usize).min(rec.len());
    if fixed_len < 14 {
        return;
    }
    let bytes = &rec[4..fixed_len];
    if bytes.len() < 10 {
        return;
    }

    // Bytes 0..8: BlobID (i64), Bytes 8..10: LobStructureType (i16)
    let lob_type = i16::from_le_bytes([bytes[8], bytes[9]]);
    match lob_type {
        // 0 = SMALL_ROOT: Bytes 10..12 = Length (u16), Bytes 16..16+Length = Data
        0 => {
            if bytes.len() >= 16 {
                let len = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
                let end = (16 + len).min(bytes.len());
                out.extend_from_slice(&bytes[16..end]);
            }
        }
        // 2 = INTERNAL: Bytes 12..14 = CurLinks (u16), starting at offset 16: 16-byte InternalLobSlotPointer
        // (Offset 8B, PageID 4B, FileID 2B, SlotID 2B)
        2 => {
            if bytes.len() >= 16 {
                let cur_links = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;
                let mut off = 16;
                for _ in 0..cur_links {
                    if off + 16 > bytes.len() {
                        break;
                    }
                    if let Some(child_ptr) = SlotPointer::parse(&bytes[off + 8..off + 16]) {
                        read_lob_slot_recursive(storage, child_ptr, depth + 1, out);
                    }
                    off += 16;
                }
            }
        }
        // 3 = DATA: Bytes 10..end = raw data bytes
        3 => {
            if bytes.len() > 10 {
                out.extend_from_slice(&bytes[10..]);
            }
        }
        // 1 | 4 | 5 = LARGE_ROOT / LARGE_ROOT_SHILOH / LARGE_ROOT_YUKON
        // Bytes 12..14 = CurLinks (u16), starting at offset 20: 12-byte LobSlotPointer
        // (Size 4B, PageID 4B, FileID 2B, SlotID 2B)
        1 | 4 | 5 => {
            if bytes.len() >= 20 {
                let cur_links = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;
                let mut off = 20;
                for _ in 0..cur_links {
                    if off + 12 > bytes.len() {
                        break;
                    }
                    if let Some(child_ptr) = SlotPointer::parse(&bytes[off + 4..off + 12]) {
                        read_lob_slot_recursive(storage, child_ptr, depth + 1, out);
                    }
                    off += 12;
                }
            }
        }
        _ => {}
    }
}
