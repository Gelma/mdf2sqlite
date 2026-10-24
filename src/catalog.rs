#![allow(dead_code)]

use crate::lob::resolve_var_bytes;
use crate::page::{BootPageInfo, MdfStorage, PagePointer};
use crate::record::ParsedRecord;
use crate::types::{decode_sql_value, decode_utf16le, fixed_type_size, SqliteValue};
use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct AllocUnit {
    pub auid: i64,
    pub au_type: u8,
    pub ownerid: i64,
    pub status: i32,
    pub fgid: u16,
    pub pg_first: Option<PagePointer>,
    pub pg_root: Option<PagePointer>,
    pub pg_first_iam: Option<PagePointer>,
}

#[derive(Clone, Debug)]
pub struct RowsetInfo {
    pub rowsetid: i64,
    pub ownertype: u8,
    pub idmajor: i32,
    pub idminor: i32,
    pub numpart: i32,
    pub status: i32,
    pub rcrows: i64,
    pub cmprlevel: u8,
}

#[derive(Clone, Debug)]
pub struct ColumnDef {
    pub colid: i32,
    pub name: String,
    pub xtype: u8,
    pub utype: i32,
    pub max_length: i16,
    pub precision: u8,
    pub scale: u8,
    pub collation_id: i32,
    pub is_nullable: bool,
    pub is_identity: bool,
    pub is_computed: bool,
}

#[derive(Clone, Debug)]
pub struct RsColInfo {
    pub rscolid: i32,
    pub hbcolid: i32,
    pub ordkey: i16,
    pub status: i32,
    pub leaf_offset: i16,
    pub leaf_nullbit: i16,
    pub leaf_bitpos: u8,
}

impl RsColInfo {
    pub fn is_dropped(&self) -> bool {
        (self.status & 0x02) != 0
    }

    pub fn is_uniqueifier(&self) -> bool {
        (self.status & 0x10) != 0
    }

    pub fn is_sparse(&self) -> bool {
        (self.status & 0x100) != 0
    }

    pub fn is_not_null(&self) -> bool {
        (self.status & 0x80) != 0
    }
}

#[derive(Clone, Debug)]
pub struct PartitionPlan {
    pub rowset: RowsetInfo,
    pub alloc_unit: AllocUnit,
    pub rscols_by_rscolid: HashMap<i32, RsColInfo>,
}

#[derive(Clone, Debug)]
pub struct IndexColumnDef {
    pub subid: i32,
    pub colid: i32,
    pub is_descending: bool,
}

#[derive(Clone, Debug)]
pub struct IndexDef {
    pub index_id: i32,
    pub name: String,
    pub idx_type: u8,
    pub is_unique: bool,
    pub is_primary_key: bool,
    pub columns: Vec<IndexColumnDef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferentialAction {
    NoAction,
    Cascade,
    SetNull,
    SetDefault,
}

impl ReferentialAction {
    pub fn from_bits(bits: i32) -> Self {
        match bits & 0x03 {
            1 => Self::Cascade,
            2 => Self::SetNull,
            3 => Self::SetDefault,
            _ => Self::NoAction,
        }
    }

    pub fn to_sql_clause(self, event: &str) -> Option<String> {
        match self {
            Self::NoAction => None,
            Self::Cascade => Some(format!("ON {} CASCADE", event)),
            Self::SetNull => Some(format!("ON {} SET NULL", event)),
            Self::SetDefault => Some(format!("ON {} SET DEFAULT", event)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ForeignKeyColumnDef {
    pub constraint_column_id: i32,
    pub parent_column_id: i32,
    pub referenced_column_id: i32,
}

#[derive(Clone, Debug)]
pub struct ForeignKeyDef {
    pub object_id: i32,
    pub name: String,
    pub parent_object_id: i32,
    pub referenced_object_id: i32,
    pub delete_action: ReferentialAction,
    pub update_action: ReferentialAction,
    pub columns: Vec<ForeignKeyColumnDef>,
}

#[derive(Clone, Debug)]
pub struct UserTablePlan {
    pub object_id: i32,
    pub schema_name: String,
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    pub pk_colids: Vec<i32>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
    pub partitions: Vec<PartitionPlan>,
}

impl UserTablePlan {
    /// Returns the output SQLite table name (`table_name` if in `dbo` schema and unique,
    /// or `schema_name.table_name` / `schema_table` if needed).
    pub fn sqlite_table_name(&self, has_duplicate_name: bool) -> String {
        if self.schema_name.eq_ignore_ascii_case("dbo") && !has_duplicate_name {
            self.table_name.clone()
        } else {
            format!("{}_{}", self.schema_name, self.table_name)
        }
    }
}

#[derive(Clone, Debug)]
pub struct DatabaseCatalog {
    pub boot: BootPageInfo,
    pub tables: Vec<UserTablePlan>,
}

impl DatabaseCatalog {
    pub fn load(storage: &MdfStorage) -> Result<Self> {
        let boot_page = storage
            .get_page(PagePointer {
                file_id: 1,
                page_id: 9,
            })
            .context("Missing Boot Page (1:9)")?;
        let boot = BootPageInfo::parse(&boot_page)?;

        // 1. Scan sys.sysallocunits (object_id = 7) starting from boot.first_sys_indexes
        let sysalloc_first = storage
            .find_btree_first_leaf(Some(boot.first_sys_indexes), Some(boot.first_sys_indexes))
            .unwrap_or(boot.first_sys_indexes);
        let sysalloc_pages = storage.collect_linked_data_pages(sysalloc_first);

        let mut alloc_units = Vec::new();
        for page_ptr in sysalloc_pages {
            let Some(page) = storage.get_page(page_ptr) else {
                continue;
            };
            for slot in 0..page.header.slot_count {
                let Some(raw) = page.slot_slice(slot) else {
                    continue;
                };
                let Some(rec) = ParsedRecord::parse(raw) else {
                    continue;
                };
                if rec.fixed_len < 4 + 41 {
                    continue;
                }
                let f = &rec.raw[4..rec.fixed_len];
                let auid = i64::from_le_bytes([f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]]);
                let au_type = f[8];
                let ownerid =
                    i64::from_le_bytes([f[9], f[10], f[11], f[12], f[13], f[14], f[15], f[16]]);
                let status = i32::from_le_bytes([f[17], f[18], f[19], f[20]]);
                let fgid = u16::from_le_bytes([f[21], f[22]]);
                let pg_first = PagePointer::parse(&f[23..29]);
                let pg_root = PagePointer::parse(&f[29..35]);
                let pg_first_iam = PagePointer::parse(&f[35..41]);

                alloc_units.push(AllocUnit {
                    auid,
                    au_type,
                    ownerid,
                    status,
                    fgid,
                    pg_first,
                    pg_root,
                    pg_first_iam,
                });
            }
        }

        // Helper to scan any system catalog table by its idmajor encoded in ownerid:
        // For system base tables: idmajor = (ownerid >> 16) as i32, idminor = (ownerid >> 48) as i16 (0 or 1)
        let find_sys_au = |target_major: i32| -> Option<&AllocUnit> {
            alloc_units.iter().find(|au| {
                au.au_type == 1
                    && ((au.ownerid >> 16) as i32) == target_major
                    && ((au.ownerid >> 48) as i16) <= 1
            })
        };

        // 2. Scan sys.sysrowsets (idmajor = 5)
        let mut rowsets = Vec::new();
        if let Some(au) = find_sys_au(5) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 35 {
                        continue;
                    }
                    let f = &rec.raw[4..rec.fixed_len];
                    let rowsetid =
                        i64::from_le_bytes([f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]]);
                    let ownertype = f[8];
                    let idmajor = i32::from_le_bytes([f[9], f[10], f[11], f[12]]);
                    let idminor = i32::from_le_bytes([f[13], f[14], f[15], f[16]]);
                    let numpart = i32::from_le_bytes([f[17], f[18], f[19], f[20]]);
                    let status = i32::from_le_bytes([f[21], f[22], f[23], f[24]]);
                    let rcrows =
                        i64::from_le_bytes([f[27], f[28], f[29], f[30], f[31], f[32], f[33], f[34]]);
                    let cmprlevel = if f.len() >= 36 { f[35] } else { 0 };

                    rowsets.push(RowsetInfo {
                        rowsetid,
                        ownertype,
                        idmajor,
                        idminor,
                        numpart,
                        status,
                        rcrows,
                        cmprlevel,
                    });
                }
            }
        }

        // Also allow finding system tables via sysrowsets in case ownerid encoding differs
        let find_au_for_sys_table = |target_major: i32| -> Option<AllocUnit> {
            if let Some(au) = find_sys_au(target_major) {
                return Some(au.clone());
            }
            let rs = rowsets
                .iter()
                .find(|r| r.idmajor == target_major && r.idminor <= 1)?;
            alloc_units
                .iter()
                .find(|au| au.au_type == 1 && au.ownerid == rs.rowsetid)
                .cloned()
        };

        // 3. Scan sys.sysowners (idmajor = 44) for schema names (default 1 -> "dbo")
        let mut schemas: HashMap<i32, String> = HashMap::new();
        schemas.insert(1, "dbo".to_string());
        schemas.insert(4, "sys".to_string());
        if let Some(au) = find_au_for_sys_table(44) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 4 {
                        continue;
                    }
                    let id = i32::from_le_bytes([rec.raw[4], rec.raw[5], rec.raw[6], rec.raw[7]]);
                    if let Some(ve) = rec.var_entry(0) {
                        let name = decode_utf16le(ve.data, true);
                        if !name.is_empty() {
                            schemas.insert(id, name);
                        }
                    }
                }
            }
        }

        // 4. Scan sys.sysschobjs (idmajor = 34) for User Tables (type == "U ") and Foreign Keys (type == "F ")
        let mut user_objs: Vec<(i32, i32, String)> = Vec::new();
        let mut raw_fks: HashMap<i32, (String, i32, i32)> = HashMap::new();
        if let Some(au) = find_au_for_sys_table(34) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 19 {
                        continue;
                    }
                    let f = &rec.raw[4..rec.fixed_len];
                    let id = i32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                    let nsid = i32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                    let status = i32::from_le_bytes([f[9], f[10], f[11], f[12]]);
                    let obj_type = &f[13..15];
                    let pid = i32::from_le_bytes([f[15], f[16], f[17], f[18]]);
                    if obj_type == b"U " {
                        if let Some(ve) = rec.var_entry(0) {
                            let name = decode_utf16le(ve.data, true);
                            if !name.is_empty() {
                                user_objs.push((id, nsid, name));
                            }
                        }
                    } else if obj_type == b"F " {
                        if let Some(ve) = rec.var_entry(0) {
                            let name = decode_utf16le(ve.data, true);
                            if !name.is_empty() {
                                raw_fks.insert(id, (name, pid, status));
                            }
                        }
                    }
                }
            }
        }

        // 5. Scan sys.syscolpars (idmajor = 41) for logical columns of tables (number == 0)
        let mut cols_by_obj: HashMap<i32, Vec<ColumnDef>> = HashMap::new();
        if let Some(au) = find_au_for_sys_table(41) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 27 {
                        continue;
                    }
                    let f = &rec.raw[4..rec.fixed_len];
                    let id = i32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                    let number = i16::from_le_bytes([f[4], f[5]]);
                    if number != 0 {
                        continue;
                    }
                    let colid = i32::from_le_bytes([f[6], f[7], f[8], f[9]]);
                    let xtype = f[10];
                    let utype = i32::from_le_bytes([f[11], f[12], f[13], f[14]]);
                    let max_length = i16::from_le_bytes([f[15], f[16]]);
                    let precision = f[17];
                    let scale = f[18];
                    let collation_id = i32::from_le_bytes([f[19], f[20], f[21], f[22]]);
                    let status = i32::from_le_bytes([f[23], f[24], f[25], f[26]]);

                    let is_nullable = (status & 0x01) == 0;
                    let is_identity = (status & 0x04) != 0;
                    let is_computed = (status & 0x10) != 0;

                    let name = rec
                        .var_entry(0)
                        .map(|ve| decode_utf16le(ve.data, true))
                        .unwrap_or_else(|| format!("col_{}", colid));

                    cols_by_obj.entry(id).or_default().push(ColumnDef {
                        colid,
                        name,
                        xtype,
                        utype,
                        max_length,
                        precision,
                        scale,
                        collation_id,
                        is_nullable,
                        is_identity,
                        is_computed,
                    });
                }
            }
        }

        // 6. Scan sys.sysrscols (idmajor = 3) for physical rowset column offsets
        let mut rscols_by_rsid: HashMap<i64, Vec<RsColInfo>> = HashMap::new();
        if let Some(au) = find_au_for_sys_table(3) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 52 {
                        continue;
                    }
                    let f = &rec.raw[4..rec.fixed_len];
                    let rsid =
                        i64::from_le_bytes([f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]]);
                    let rscolid = i32::from_le_bytes([f[8], f[9], f[10], f[11]]);
                    let hbcolid = i32::from_le_bytes([f[12], f[13], f[14], f[15]]);
                    let ordkey = i16::from_le_bytes([f[32], f[33]]);
                    let status = i32::from_le_bytes([f[36], f[37], f[38], f[39]]);
                    let offset_raw = i32::from_le_bytes([f[40], f[41], f[42], f[43]]);
                    let nullbit_raw = i32::from_le_bytes([f[44], f[45], f[46], f[47]]);
                    let bitpos_raw = i32::from_le_bytes([f[48], f[49], f[50], f[51]]);

                    let leaf_offset = (offset_raw & 0xFFFF) as i16;
                    let leaf_nullbit = (nullbit_raw & 0xFFFF) as i16;
                    let leaf_bitpos = (bitpos_raw & 0xFF) as u8;

                    rscols_by_rsid.entry(rsid).or_default().push(RsColInfo {
                        rscolid,
                        hbcolid,
                        ordkey,
                        status,
                        leaf_offset,
                        leaf_nullbit,
                        leaf_bitpos,
                    });
                }
            }
        }

        // 7. Scan sys.siscols (idmajor = 55) for index key columns
        // Layout: idmajor (4B, 0..4), idminor/indid (4B, 4..8), subid (4B, 8..12),
        //         status (4B, 12..16), intprop/colid (4B, 16..20), tinyprop1 (1B, 20)
        let mut idx_cols_by_obj_ind: HashMap<(i32, i32), Vec<IndexColumnDef>> = HashMap::new();
        if let Some(au) = find_au_for_sys_table(55) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 21 {
                        continue;
                    }
                    let f = &rec.raw[4..rec.fixed_len];
                    let idmajor = i32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                    let idminor = i32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                    let subid = i32::from_le_bytes([f[8], f[9], f[10], f[11]]);
                    let status = i32::from_le_bytes([f[12], f[13], f[14], f[15]]);
                    let colid = i32::from_le_bytes([f[16], f[17], f[18], f[19]]);
                    let tp1 = f[20];

                    // Key columns in real indexes have bit 0x01 set and tp1 > 0 (exclude INCLUDEd non-key columns)
                    let is_key_col = (status & 0x01) != 0 && (status & 0x10) == 0 && tp1 > 0;
                    if !is_key_col || colid <= 0 {
                        continue;
                    }
                    let is_descending = (status & 0x04) != 0;

                    idx_cols_by_obj_ind
                        .entry((idmajor, idminor))
                        .or_default()
                        .push(IndexColumnDef {
                            subid,
                            colid,
                            is_descending,
                        });
                }
            }
        }

        // 8. Scan sys.sysidxstats (idmajor = 54) for Clustered (type=1) and Non-Clustered (type=2) indexes
        // Layout: id (4B, 0..4), indid (4B, 4..8), status (4B, 8..12), intprop (4B, 12..16),
        //         fillfact (1B, 16), type (1B, 17), tinyprop (1B, 18), ...; var[0] = name (UTF-16LE)
        let mut indexes_by_obj: HashMap<i32, Vec<IndexDef>> = HashMap::new();
        if let Some(au) = find_au_for_sys_table(54) {
            let pages =
                storage.collect_partition_data_pages(au.pg_first, au.pg_root, au.pg_first_iam, true);
            for page_ptr in pages {
                let Some(page) = storage.get_page(page_ptr) else {
                    continue;
                };
                for slot in 0..page.header.slot_count {
                    let Some(raw) = page.slot_slice(slot) else {
                        continue;
                    };
                    let Some(rec) = ParsedRecord::parse(raw) else {
                        continue;
                    };
                    if rec.fixed_len < 4 + 19 {
                        continue;
                    }
                    let f = &rec.raw[4..rec.fixed_len];
                    let id = i32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                    let indid = i32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                    let status = i32::from_le_bytes([f[8], f[9], f[10], f[11]]);
                    let idx_type = f[17];

                    // Only migrate active Clustered (1) and Non-Clustered (2) indexes
                    if (idx_type != 1 && idx_type != 2) || indid <= 0 {
                        continue;
                    }
                    // Skip disabled (0x80) or hypothetical (0x20 in some versions? No, 0x20 is PK; hypothetical has status & 0x10 != 0 or no rowset)
                    if (status & 0x80) != 0 {
                        continue;
                    }

                    let Some(ve) = rec.var_entry(0) else {
                        continue;
                    };
                    let name = decode_utf16le(ve.data, true);
                    if name.is_empty() {
                        continue;
                    }

                    let mut idx_cols = idx_cols_by_obj_ind
                        .remove(&(id, indid))
                        .unwrap_or_default();
                    if idx_cols.is_empty() {
                        continue;
                    }
                    idx_cols.sort_by_key(|c| c.subid);

                    let is_unique = (status & 0x08) != 0;
                    let is_primary_key = (status & 0x20) != 0;

                    indexes_by_obj.entry(id).or_default().push(IndexDef {
                        index_id: indid,
                        name,
                        idx_type,
                        is_unique,
                        is_primary_key,
                        columns: idx_cols,
                    });
                }
            }
        }

        // 9. Scan sys.syssingleobjrefs (idmajor = 74) and sys.sysmultiobjrefs (idmajor = 75)
        // to resolve Foreign Key referenced tables (class = 27) and column pairs (class = 28 parent, class = 29 referenced).
        // Fixed layout (21 bytes): class (1B, 0), depid (4B, 1..5), depsubid (4B, 5..9),
        //                          indepid (4B, 9..13), indepsubid (4B, 13..17), status (4B, 17..21)
        let mut fk_ref_table: HashMap<i32, i32> = HashMap::new();
        let mut fk_parent_cols: HashMap<i32, HashMap<i32, i32>> = HashMap::new();
        let mut fk_ref_cols: HashMap<i32, HashMap<i32, i32>> = HashMap::new();

        if !raw_fks.is_empty() {
            for sys_ref_id in [74, 75] {
                if let Some(au) = find_au_for_sys_table(sys_ref_id) {
                    let pages = storage.collect_partition_data_pages(
                        au.pg_first,
                        au.pg_root,
                        au.pg_first_iam,
                        true,
                    );
                    for page_ptr in pages {
                        let Some(page) = storage.get_page(page_ptr) else {
                            continue;
                        };
                        for slot in 0..page.header.slot_count {
                            let Some(raw) = page.slot_slice(slot) else {
                                continue;
                            };
                            let Some(rec) = ParsedRecord::parse(raw) else {
                                continue;
                            };
                            if rec.fixed_len < 4 + 21 {
                                continue;
                            }
                            let f = &rec.raw[4..rec.fixed_len];
                            let cls = f[0];
                            if cls != 27 && cls != 28 && cls != 29 {
                                continue;
                            }
                            let depid = i32::from_le_bytes([f[1], f[2], f[3], f[4]]);
                            if !raw_fks.contains_key(&depid) {
                                continue;
                            }
                            let depsubid = i32::from_le_bytes([f[5], f[6], f[7], f[8]]);
                            let indepid = i32::from_le_bytes([f[9], f[10], f[11], f[12]]);
                            let indepsubid = i32::from_le_bytes([f[13], f[14], f[15], f[16]]);

                            match cls {
                                27 => {
                                    fk_ref_table.insert(depid, indepid);
                                }
                                28 => {
                                    fk_parent_cols
                                        .entry(depid)
                                        .or_default()
                                        .insert(depsubid, indepsubid);
                                }
                                29 => {
                                    fk_ref_cols
                                        .entry(depid)
                                        .or_default()
                                        .insert(depsubid, indepsubid);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        let mut fks_by_parent_obj: HashMap<i32, Vec<ForeignKeyDef>> = HashMap::new();
        for (fk_id, (fk_name, parent_obj_id, status)) in raw_fks {
            let Some(&referenced_obj_id) = fk_ref_table.get(&fk_id) else {
                continue;
            };
            let Some(pcols) = fk_parent_cols.get(&fk_id) else {
                continue;
            };
            let Some(rcols) = fk_ref_cols.get(&fk_id) else {
                continue;
            };

            let mut subids: Vec<i32> = pcols.keys().copied().collect();
            subids.sort();

            let mut fk_columns = Vec::with_capacity(subids.len());
            for subid in subids {
                if let (Some(&pcol), Some(&rcol)) = (pcols.get(&subid), rcols.get(&subid)) {
                    fk_columns.push(ForeignKeyColumnDef {
                        constraint_column_id: subid,
                        parent_column_id: pcol,
                        referenced_column_id: rcol,
                    });
                }
            }

            if fk_columns.is_empty() {
                continue;
            }

            let delete_action = ReferentialAction::from_bits(status >> 12);
            let update_action = ReferentialAction::from_bits(status >> 14);

            fks_by_parent_obj
                .entry(parent_obj_id)
                .or_default()
                .push(ForeignKeyDef {
                    object_id: fk_id,
                    name: fk_name,
                    parent_object_id: parent_obj_id,
                    referenced_object_id: referenced_obj_id,
                    delete_action,
                    update_action,
                    columns: fk_columns,
                });
        }

        // Index IN_ROW_DATA alloc units by ownerid (rowsetid)
        let mut inrow_au_by_rsid: HashMap<i64, AllocUnit> = HashMap::new();
        for au in &alloc_units {
            if au.au_type == 1 {
                inrow_au_by_rsid.insert(au.ownerid, au.clone());
            }
        }

        // Group rowsets by idmajor (object_id) for Heap (idminor == 0) or Clustered Index (idminor == 1)
        let mut rowsets_by_obj: HashMap<i32, Vec<RowsetInfo>> = HashMap::new();
        for rs in rowsets {
            if rs.idminor == 0 || rs.idminor == 1 {
                rowsets_by_obj.entry(rs.idmajor).or_default().push(rs);
            }
        }

        // Sort user objects by name
        user_objs.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));

        let mut tables = Vec::with_capacity(user_objs.len());
        for (obj_id, nsid, table_name) in user_objs {
            let schema_name = schemas
                .get(&nsid)
                .cloned()
                .unwrap_or_else(|| format!("schema_{}", nsid));

            let mut columns = cols_by_obj.remove(&obj_id).unwrap_or_default();
            columns.sort_by_key(|c| c.colid);
            if columns.is_empty() {
                continue;
            }

            let mut partitions = Vec::new();
            let mut pk_colids = Vec::new();

            if let Some(mut obj_rowsets) = rowsets_by_obj.remove(&obj_id) {
                // If somehow both idminor 0 and 1 exist, prefer idminor 1 (Clustered Index)
                let has_clustered = obj_rowsets.iter().any(|r| r.idminor == 1);
                if has_clustered {
                    obj_rowsets.retain(|r| r.idminor == 1);
                }

                for rs in obj_rowsets {
                    let Some(au) = inrow_au_by_rsid.get(&rs.rowsetid).cloned() else {
                        continue;
                    };
                    let rscols = rscols_by_rsid.remove(&rs.rowsetid).unwrap_or_default();

                    if pk_colids.is_empty() && rs.idminor == 1 {
                        // Extract clustered index key columns (excluding any hidden uniqueifier)
                        let has_uniqueifier = rscols.iter().any(|rc| rc.is_uniqueifier());
                        if !has_uniqueifier {
                            let mut key_cols: Vec<(i16, i32)> = rscols
                                .iter()
                                .filter(|rc| {
                                    rc.ordkey > 0 && rc.rscolid > 0 && !rc.is_dropped()
                                })
                                .map(|rc| (rc.ordkey, rc.rscolid))
                                .collect();
                            key_cols.sort_by_key(|k| k.0);
                            pk_colids = key_cols.into_iter().map(|k| k.1).collect();
                        }
                    }

                    let mut rscols_by_rscolid = HashMap::with_capacity(rscols.len());
                    for rc in rscols {
                        if !rc.is_dropped() && !rc.is_uniqueifier() && rc.rscolid > 0 {
                            // Update column nullability from sysrscols if available
                            if let Some(col) = columns.iter_mut().find(|c| c.colid == rc.rscolid) {
                                col.is_nullable = !rc.is_not_null();
                            }
                            rscols_by_rscolid.insert(rc.rscolid, rc);
                        }
                    }

                    partitions.push(PartitionPlan {
                        rowset: rs,
                        alloc_unit: au,
                        rscols_by_rscolid,
                    });
                }
            }

            let mut indexes = indexes_by_obj.remove(&obj_id).unwrap_or_default();
            indexes.sort_by_key(|idx| idx.index_id);

            let mut foreign_keys = fks_by_parent_obj.remove(&obj_id).unwrap_or_default();
            foreign_keys.sort_by(|a, b| a.name.cmp(&b.name).then(a.object_id.cmp(&b.object_id)));

            tables.push(UserTablePlan {
                object_id: obj_id,
                schema_name,
                table_name,
                columns,
                pk_colids,
                indexes,
                foreign_keys,
                partitions,
            });
        }

        Ok(Self { boot, tables })
    }
}

/// Extracts all column values for a single `ParsedRecord` according to the table's `columns` and `PartitionPlan`.
pub fn extract_row_values(
    storage: &MdfStorage,
    rec: &ParsedRecord<'_>,
    columns: &[ColumnDef],
    part: &PartitionPlan,
    default_collation_id: i32,
    out_row: &mut Vec<SqliteValue>,
) {
    out_row.clear();
    for col in columns {
        let Some(rc) = part.rscols_by_rscolid.get(&col.colid) else {
            out_row.push(SqliteValue::Null);
            continue;
        };

        if rc.is_sparse() {
            if let Some(ref sv) = rec.sparse_vector {
                if let Some(entry) = sv.get_column(col.colid as u16) {
                    if entry.is_complex {
                        let resolved = resolve_var_bytes(storage, &entry);
                        out_row.push(decode_sql_value(
                            col.xtype,
                            &resolved,
                            col.scale,
                            rc.leaf_bitpos,
                            col.collation_id,
                            default_collation_id,
                        ));
                    } else {
                        out_row.push(decode_sql_value(
                            col.xtype,
                            entry.data,
                            col.scale,
                            rc.leaf_bitpos,
                            col.collation_id,
                            default_collation_id,
                        ));
                    }
                    continue;
                }
            }
            out_row.push(SqliteValue::Null);
            continue;
        }

        if rec.is_null(rc.leaf_nullbit) {
            out_row.push(SqliteValue::Null);
            continue;
        }

        if rc.leaf_offset > 0 {
            let off = rc.leaf_offset as usize;
            let len = fixed_type_size(col.xtype, col.max_length);
            if let Some(bytes) = rec.fixed_slice(off, len) {
                out_row.push(decode_sql_value(
                    col.xtype,
                    bytes,
                    col.scale,
                    rc.leaf_bitpos,
                    col.collation_id,
                    default_collation_id,
                ));
            } else {
                out_row.push(SqliteValue::Null);
            }
        } else if rc.leaf_offset < 0 {
            let var_idx = ((-rc.leaf_offset) - 1) as usize;
            if let Some(entry) = rec.var_entry(var_idx) {
                if entry.is_complex {
                    let resolved = resolve_var_bytes(storage, &entry);
                    out_row.push(decode_sql_value(
                        col.xtype,
                        &resolved,
                        col.scale,
                        rc.leaf_bitpos,
                        col.collation_id,
                        default_collation_id,
                    ));
                } else {
                    out_row.push(decode_sql_value(
                        col.xtype,
                        entry.data,
                        col.scale,
                        rc.leaf_bitpos,
                        col.collation_id,
                        default_collation_id,
                    ));
                }
            } else {
                out_row.push(SqliteValue::Null);
            }
        } else {
            out_row.push(SqliteValue::Null);
        }
    }
}
