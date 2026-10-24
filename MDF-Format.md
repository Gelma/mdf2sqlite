# Complete Technical Guide to the Microsoft SQL Server Binary Format (.MDF, .NDF, .LDF, .BAK)
> **Purpose of this document**: Provide an exhaustive engineering specification, detailed down to individual bytes, offsets, bitmasks, and decoding algorithms, to implement a standalone cross-platform engine (Linux and Windows) capable of reading, navigating, and extracting data directly from physical Microsoft SQL Server files without requiring a running SQL Server instance.
> **Analyzed sources**: *OrcaMDF*, *OrcaSql*, *mdf-rs*, *mtf-rs*, *SQLServerForensics*, *oxidized-mdf*, *mssql_4n6*, *internals-viewer-danny*, *improve.dk* (Mark S. Rasmussen reverse engineering series), *MSSQL-mdf-file-dump*, *MSSQLToSQLite*.

---

## Table of Contents
1. [General File Architecture and Physical Storage](#1-general-file-architecture-and-physical-storage)
   - 1.1 [File Types (.MDF, .NDF, .LDF, .BAK)](#11-file-types-mdf-ndf-ldf-bak)
   - 1.2 [Pages, Extents, and Allocation Units](#12-pages-extents-and-allocation-units)
   - 1.3 [Page Pointers and Record Identifiers (RID)](#13-page-pointers-and-record-identifiers-rid)
2. [Physical Page Layout (8192 Bytes)](#2-physical-page-layout-8192-bytes)
   - 2.1 [Three-Part Page Structure](#21-three-part-page-structure)
   - 2.2 [Slot Array (Row Offset Table)](#22-slot-array-row-offset-table)
3. [Page Header (96 Bytes): Detailed Field-by-Field Specification](#3-page-header-96-bytes-detailed-field-by-field-specification)
   - 3.1 [Complete Header Field Table](#31-complete-header-field-table)
   - 3.2 [Detailed Breakdown of m_flagBits (Offset 4)](#32-detailed-breakdown-of-m_flagbits-offset-4)
   - 3.3 [Allocation Unit ID (AUID) Resolution](#33-allocation-unit-id-auid-resolution)
4. [Page Integrity and Sector Protection (Torn Page & Checksum)](#4-page-integrity-and-sector-protection-torn-page--checksum)
   - 4.1 [Torn Page Detection (Flag 0x0100)](#41-torn-page-detection-flag-0x0100)
   - 4.2 [Page Checksum (Flag 0x0200)](#42-page-checksum-flag-0x0200)
5. [Page Types and Allocation Maps](#5-page-types-and-allocation-maps)
   - 5.1 [File Header Page (Type 15 / 0x0F - Page 1:0)](#51-file-header-page-type-15--0x0f---page-10)
   - 5.2 [Boot Page (Type 13 / 0x0D - Page 1:9)](#52-boot-page-type-13--0x0d---page-19)
   - 5.3 [PFS - Page Free Space (Type 11 / 0x0B)](#53-pfs---page-free-space-type-11--0x0b)
   - 5.4 [GAM and SGAM (Types 8 and 9 / 0x08, 0x09)](#54-gam-and-sgam-types-8-and-9--0x08-0x09)
   - 5.5 [DCM and BCM (Types 16 and 17 / 0x10, 0x11)](#55-dcm-and-bcm-types-16-and-17--0x10-0x11)
   - 5.6 [IAM - Index Allocation Map (Type 10 / 0x0A)](#56-iam---index-allocation-map-type-10--0x0a)
6. [Data Record Format (FixedVar Format)](#6-data-record-format-fixedvar-format)
   - 6.1 [General Structure and Record Header (Status Bits A & B)](#61-general-structure-and-record-header-status-bits-a--b)
   - 6.2 [Fixed-Length Data Section](#62-fixed-length-data-section)
   - 6.3 [Null Bitmap](#63-null-bitmap)
   - 6.4 [Variable-Length Section and Offset Array](#64-variable-length-section-and-offset-array)
   - 6.5 [Complex Columns and the 0x8000 Bit Flag](#65-complex-columns-and-the-0x8000-bit-flag)
   - 6.6 [Forwarding Stub and Back Pointer (Heap Fragmentation)](#66-forwarding-stub-and-back-pointer-heap-fragmentation)
   - 6.7 [Sparse Columns and Sparse Vector](#67-sparse-columns-and-sparse-vector)
7. [Data Compression: Row & Page Compression (CD Format)](#7-data-compression-row--page-compression-cd-format)
   - 7.1 [CD Record Header and Column Descriptors (4 Bits per Column)](#71-cd-record-header-and-column-descriptors-4-bits-per-column)
   - 7.2 [Short Data Region and Cluster Array (30 Columns per Cluster)](#72-short-data-region-and-cluster-array-30-columns-per-cluster)
   - 7.3 [Long Data Region and Offset Array](#73-long-data-region-and-offset-array)
   - 7.4 [Page Compression: Compression Info (CI), Anchor Record, and Dictionary](#74-page-compression-compression-info-ci-anchor-record-and-dictionary)
   - 7.5 [Decoding Compressed Integers: Big-Endian and Inverted Sign Bit](#75-decoding-compressed-integers-big-endian-and-inverted-sign-bit)
   - 7.6 [Unicode Compression (SCSU - Standard Compression Scheme for Unicode)](#76-unicode-compression-scsu---standard-compression-scheme-for-unicode)
8. [B-Tree Architecture and Index Records](#8-b-tree-architecture-and-index-records)
   - 8.1 [Root, Intermediate, and Leaf Nodes](#81-root-intermediate-and-leaf-nodes)
   - 8.2 [Clustered vs. Non-Clustered Indexes (Heap vs. B-Tree)](#82-clustered-vs-non-clustered-indexes-heap-vs-b-tree)
   - 8.3 [Down-Page Pointer in Intermediate Nodes](#83-down-page-pointer-in-intermediate-nodes)
   - 8.4 [Uniquifier Handling in Non-Unique Clustered Indexes](#84-uniquifier-handling-in-non-unique-clustered-indexes)
   - 8.5 [RID Pointers vs. Clustered Keys in Non-Clustered Indexes](#85-rid-pointers-vs-clustered-keys-in-non-clustered-indexes)
9. [Large and Out-of-Row Data Management (LOB & Row-Overflow)](#9-large-and-out-of-row-data-management-lob--row-overflow)
   - 9.1 [In-Row Pointers: Blob Inline Root (12..72 Bytes) and Text Pointer (16 Bytes)](#91-in-row-pointers-blob-inline-root-1272-bytes-and-text-pointer-16-bytes)
   - 9.2 [LOB Pages (Type 3 / 0x03) and Text Tree Structures](#92-lob-pages-type-3--0x03-and-text-tree-structures)
   - 9.3 [LOB Node Types: SMALL_ROOT, LARGE_ROOT_YUKON, INTERNAL, DATA](#93-lob-node-types-small_root-large_root_yukon-internal-data)
   - 9.4 [Recursive LOB Stream Reassembly Algorithm](#94-recursive-lob-stream-reassembly-algorithm)
10. [Binary Encoding and Decoding of All SQL Server Data Types](#10-binary-encoding-and-decoding-of-all-sql-server-data-types)
    - 10.1 [Integer, Fixed-Point, and Floating-Point Numeric Types](#101-integer-fixed-point-and-floating-point-numeric-types)
    - 10.2 [Text Strings and Binary Types (Varchar, Nvarchar, Collation)](#102-text-strings-and-binary-types-varchar-nvarchar-collation)
    - 10.3 [Dates and Times (DateTime, SmallDateTime, Date, Time, DateTime2, DateTimeOffset)](#103-dates-and-times-datetime-smalldatetime-date-time-datetime2-datetimeoffset)
    - 10.4 [UniqueIdentifier (GUID)](#104-uniqueidentifier-guid)
    - 10.5 [SqlVariant](#105-sqlvariant)
    - 10.6 [Complex and Spatial Types (CLR UDT, HierarchyId, Geometry, Geography, XML)](#106-complex-and-spatial-types-clr-udt-hierarchyid-geometry-geography-xml)
11. [System Catalog Bootstrapping (System Base Tables)](#11-system-catalog-bootstrapping-system-base-tables)
    - 11.1 [Initialization Chain (Boot Page -> sysallocunits)](#111-initialization-chain-boot-page---sysallocunits)
    - 11.2 [Core System Base Tables](#112-core-system-base-tables)
    - 11.3 [Complete Schema Resolution Algorithm](#113-complete-schema-resolution-algorithm)
12. [Multi-File Database Management (.MDF + .NDF) and Filegroups](#12-multi-file-database-management-mdf--ndf-and-filegroups)
    - 12.1 [Filegroup Architecture and Proportional Fill Allocation](#121-filegroup-architecture-and-proportional-fill-allocation)
    - 12.2 [Cross-File Pointer Resolution](#122-cross-file-pointer-resolution)
13. [Related Formats: Transaction Log (.LDF) and Backups (.BAK / MTF)](#13-related-formats-transaction-log-ldf-and-backups-bak--mtf)
    - 13.1 [Transaction Log (.LDF) and VLF Architecture](#131-transaction-log-ldf-and-vlf-architecture)
    - 13.2 [Microsoft Tape Format Backup (.BAK / MTF)](#132-microsoft-tape-format-backup-bak--mtf)
14. [Conversion Patterns to Other Engines (e.g., SQLite)](#14-conversion-patterns-to-other-engines-eg-sqlite)
    - 14.1 [SQL Server to SQLite Type Mapping](#141-sql-server-to-sqlite-type-mapping)
    - 14.2 [Constraints, Trigger-Based Foreign Keys, and Indexes](#142-constraints-trigger-based-foreign-keys-and-indexes)
    - 14.3 [High-Performance Bulk Insertion Strategies](#143-high-performance-bulk-insertion-strategies)
15. [Implementation Guide for a Cross-Platform Parser (Linux & Windows)](#15-implementation-guide-for-a-cross-platform-parser-linux--windows)
    - 15.1 [Layered Parser Architecture](#151-layered-parser-architecture)
    - 15.2 [High-Performance I/O: Memory Mapping vs. Paged Streaming](#152-high-performance-io-memory-mapping-vs-paged-streaming)
    - 15.3 [Endianness Handling and Packed Structures](#153-endianness-handling-and-packed-structures)
    - 15.4 [Compliance Checklist and Integrity Tests](#154-compliance-checklist-and-integrity-tests)

---

## 1. General File Architecture and Physical Storage

A Microsoft SQL Server database stores its data and allocation structures inside one or more physical files on the file system:

### 1.1 File Types (.MDF, .NDF, .LDF, .BAK)
* **Primary Data File (.MDF - Master Data File)**: The primary file of the database. It contains the startup pointer (*Boot Page*), initial system table definitions, primary allocation maps, and user data. It is mandatory and exactly one exists per database. Its logical `FileID` is strictly **1**.
* **Secondary Data Files (.NDF - Non-master Data File)**: Optional secondary data files. They are used to distribute data across different physical disks or group objects into dedicated *Filegroups*. They share the exact same internal 8 KB page structure as the `.mdf` file. Their `FileID` values are progressive integers ($2, 3, \dots, 32767$).
* **Transaction Log File (.LDF - Log Data File)**: Contains the sequence of transaction log records structured into virtual log files (*VLFs - Virtual Log Files*). Used to implement the ARIES / WAL (*Write-Ahead Logging*) protocol. Its `FileID` is typically 2 for single-file databases or a progressive integer.
* **Backup File (.BAK / MTF)**: Container encapsulated according to the *Microsoft Tape Format* (MTF) standard. Inside resides a sequential stream of blocks that includes the raw image of allocated pages from MDF and NDF files, plus the transaction log records needed to recover to a consistent state.

### 1.2 Pages, Extents, and Allocation Units
1. **Page**:
   - The fundamental, indivisible unit of storage and I/O.
   - Has a fixed, immutable size of **8,192 bytes (8 KB)**.
   - Any `.mdf` or `.ndf` file on disk has a size that is an exact integer multiple of 8,192 bytes:
     $$\text{File Size} = N \times 8192 \text{ bytes}$$
2. **Extent**:
   - A contiguous group of **8 consecutive pages** (total size: $8 \times 8192 = 65,536 \text{ bytes} = 64 \text{ KB}$).
   - **Uniform Extent**: All 8 pages belong exclusively to a single object (table or index).
   - **Mixed Extent**: The 8 pages may be shared across up to 8 different objects. Historically used by SQL Server for the first 8 pages of a table or index to avoid wasting space on very small tables.
3. **Allocation Unit (AU)**:
   - Logical set of pages managed as a single entity by a partition.
   - Each partition of a table/index can have up to 3 allocation units:
     - `IN_ROW_DATA` (Type 1): Regular data and index records.
     - `LOB_DATA` (Type 2): Large binary/text objects (`VARCHAR(MAX)`, `VARBINARY(MAX)`, `TEXT`, `IMAGE`, `XML`).
     - `ROW_OVERFLOW_DATA` (Type 3): Variable-length columns that exceed the 8,060-byte row limit (`VARCHAR`, `NVARCHAR`, `VARBINARY` that overflow the page without being explicitly `MAX`).

### 1.3 Page Pointers and Record Identifiers (RID)
Within internal metadata and row/index pointers, SQL Server addresses structures using exact physical pointers:

* **Page Address (6 bytes)**:
  - `m_pageId` (4 bytes, Little-Endian): Sequential page number within its respective file ($0$ to $2^{32}-1$).
  - `m_fileId` (2 bytes, Little-Endian): Unique file ID within the database ($1$ for the primary `.mdf` file).
  - Physical byte offset calculation on disk within the target file:
    $$\text{Physical Offset} = \text{m\_pageId} \times 8192$$

* **Row Identifier (RID - 8 bytes)**:
  - Uniquely identifies a record across the entire database (used in Heap tables and non-clustered indexes):
    - `PageID` (4 bytes, Little-Endian, offset 0..3)
    - `FileID` (2 bytes, Little-Endian, offset 4..5)
    - `SlotID` (2 bytes, Little-Endian, offset 6..7): Index of the slot in the array at the end of the page ($0$ to `m_slotCnt - 1`).

---

## 2. Physical Page Layout (8192 Bytes)

Every 8,192-byte page is strictly divided into three contiguous sections:

```
+-------------------------------------------------------------------------+
| Bytes 0 .. 95     : PAGE HEADER (96 Bytes)                              |
|                     Page metadata (type, ID, LSN, pointers...)          |
+-------------------------------------------------------------------------+
| Bytes 96 .. FreeData : DATA RECORDS PAYLOAD                             |
|                     Records grow downward                               |
|                     (from offset 96 toward the end of the page)         |
+ - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - +
|                     FREE SPACE                                          |
|                     From m_freeData up to (8192 - 2 * m_slotCnt)        |
+ - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - +
| Bytes (8192 - 2*SlotCnt) .. 8191 : SLOT ARRAY (Row Offset Table)        |
|                     Array of 16-bit integers (2 bytes per record)       |
|                     Grows backward from the bottom of the page upward   |
+-------------------------------------------------------------------------+
```

### 2.1 Three-Part Page Structure
1. **Page Header**: 96 bytes at the beginning of the page ($[0, 95]$).
2. **Record Space and User Data**: Starts immediately after the header at offset 96 (`0x60`). Each record is written at the offset pointed to by the `m_freeData` header field.
3. **Slot Array**: Located at the very end of the page ($[8192 - 2 \times \text{m\_slotCnt}, 8191]$).

### 2.2 Slot Array (Row Offset Table)
- The Slot Array is a table of 2-byte pointers (`uint16` Little-Endian).
- Each array element corresponds to a record (or slot) numbered from $0$ to $\text{m\_slotCnt} - 1$.
- The formula to calculate the physical offset in the 8,192-byte buffer for slot $i$ is:
  $$\text{SlotArrayOffset}(i) = 8192 - 2 \times (i + 1)$$
- The 2-byte value read at this address represents the **byte offset relative to the start of the page** where the record body begins:
  $$\text{RecordStartOffset} = \text{ReadUInt16LE}(\text{PageBuffer}, \text{SlotArrayOffset}(i))$$
- **Special Slot Conventions**:
  - `RecordStartOffset == 0`: Deleted or unused slot.
  - If the page is compressed (`m_flagBits & 0x0080 != 0` or `m_flagBits & 0x0008 != 0`), **Slot 0** points to the internal `Compression Info` (CI) structure, while user data records start at Slot 1.

---

## 3. Page Header (96 Bytes): Detailed Field-by-Field Specification

All multi-byte numeric fields in the page header are encoded in **Little-Endian**.

### 3.1 Complete Header Field Table

| Offset | Length | Field Name | C/Rust Type | Functional Description |
|:---:|:---:|:---|:---|:---|
| **0** | 1 byte | `m_headerVersion` | `uint8` | Page header version. Strictly **1** from SQL Server 7.0 through SQL Server 2025. |
| **1** | 1 byte | `m_type` | `uint8` | Functional page type (1=Data, 2=Index, 3=LOB/Text, 8=GAM, 9=SGAM, 10=IAM, 11=PFS, 13=Boot, 15=FileHeader, 16=DCM, 17=BCM). |
| **2** | 1 byte | `m_typeFlagBits` | `uint8` | Secondary type flags. For data pages: bit `0x01` indicates ghost records; for LOB pages it indicates tree node characteristics. |
| **3** | 1 byte | `m_level` | `uint8` | Level in the B-Tree. **0** = Leaf (Leaf Data or Leaf Index); **> 0** = Intermediate nodes; highest level is the root. Always **0** for Heap tables. |
| **4** | 2 bytes | `m_flagBits` | `uint16` | Page status bitmask (checksum, torn page protection, compression...). |
| **6** | 2 bytes | `m_indexId` | `uint16` | Internal index or allocation structure ID (0 = Heap, 1 = Clustered Index, 2..250 = Non-Clustered Index, 255 = LOB Data). |
| **8** | 4 bytes | `m_prevPage_pageId` | `uint32` | PageID of the previous page in the B-Tree chain at this level (0 if first page). |
| **12** | 2 bytes | `m_prevPage_fileId` | `uint16` | FileID of the previous page in the B-Tree chain. |
| **14** | 2 bytes | `pminlen` | `uint16` | Size in bytes of the fixed-length portion of all records stored on this page. |
| **16** | 4 bytes | `m_nextPage_pageId` | `uint32` | PageID of the next page in the B-Tree chain at this level (0 if last page). |
| **20** | 2 bytes | `m_nextPage_fileId` | `uint16` | FileID of the next page in the B-Tree chain. |
| **22** | 2 bytes | `m_slotCnt` | `uint16` | Total number of slots allocated in the array at the end of the page. |
| **24** | 4 bytes | `m_objId` | `int32` | Metadata Object ID / Allocation Unit owner ID. |
| **28** | 2 bytes | `m_freeCnt` | `uint16` | Total number of unallocated free bytes on the page. |
| **30** | 2 bytes | `m_freeData` | `uint16` | Byte offset relative to the start of the page of the first free byte after the last record. |
| **32** | 4 bytes | `m_pageId_pageId` | `uint32` | This page's own PageID. Must match the physical page index in the file ($\text{Offset} / 8192$). |
| **36** | 2 bytes | `m_pageId_fileId` | `uint16` | FileID of the file containing this page. |
| **38** | 2 bytes | `m_reservedCnt` | `uint16` | Number of bytes reserved by active open transactions. |
| **40** | 10 bytes | `m_lsn` | `uint8[10]` | Log Sequence Number (LSN) of the last operation that modified the page. Structure: `VLF_ID` (`uint32`), `Offset` (`uint32`), `Slot` (`uint16`). |
| **50** | 2 bytes | `m_xactReserved` | `uint16` | Bytes reserved for the most recent active transaction. |
| **52** | 4 bytes | `m_xdesId_partHigh` | `uint32` | High part of the Transaction Descriptor ID (XDES). |
| **56** | 2 bytes | `m_xdesId_partLow` | `uint16` | Low part of the Transaction Descriptor ID (XDES). |
| **58** | 2 bytes | `m_ghostRecCnt` | `uint16` | Count of ghost records on the page (logically deleted but not yet physically reclaimed). |
| **60** | 4 bytes | `m_tornBits` | `uint32` | Integrity verification signature. If `m_flagBits & 0x0100`: stores the 32-bit torn page sector bits. If `m_flagBits & 0x0200`: stores the CRC32 Page Checksum. |
| **64** | 32 bytes | `reserved` | `uint8[32]` | Reserved bytes for internal alignment and engine structures. |

### 3.2 Detailed Breakdown of m_flagBits (Offset 4)
The 16-bit word at offset 4 governs the physical properties and state of the page:

| Bitmask | Constant Name | Technical Meaning |
|:---:|:---|:---|
| `0x0001` | `PAGE_ALLOCATED` | Page is properly allocated and active in the database. |
| `0x0002` | `PAGE_SYSTEM` | Page belongs to critical structures or system tables. |
| `0x0004` | `PAGE_INDEX` | Page belongs to an index (clustered or non-clustered). |
| `0x0008` | `PAGE_ROW_COMPRESSION` | Records on the page are compressed with **Row Compression** (using *CD Format* instead of *FixedVar*). |
| `0x0010` | `PAGE_BULK_LOGGED` | Bulk-load operations partially logged. |
| `0x0080` | `PAGE_PAGE_COMPRESSION` | Page is compressed with **Page Compression** (contains *Compression Info*, *Anchor Record*, and *Dictionary*). |
| `0x0100` | `PAGE_TORN_BITS` | **Torn Page Detection enabled**. The `m_tornBits` field contains the saved sector bits. |
| `0x0200` | `PAGE_CHECKSUM` | **Page Checksum enabled**. The `m_tornBits` field stores the checksum of the entire page. |

### 3.3 Allocation Unit ID (AUID) Resolution
To trace a page back to its owning table or index in the system catalogs (`sys.sysallocunits`), combine `m_objId` and `m_indexId`:
$$\text{AllocationUnitId} = (\text{uint64}(m\_indexId) \ll 48) \mid (\text{uint64}(m\_objId) \ll 16)$$
This 64-bit key allows instant lookup of the corresponding record in `sys.sysallocunits.auid`.

---

## 4. Page Integrity and Sector Protection (Torn Page & Checksum)

Before reading or extracting any records from the 8,192-byte page buffer, an offline parser must verify physical integrity and, when Torn Page Detection is active, **restore the original sector bits**.

### 4.1 Torn Page Detection (Flag 0x0100)
Hard drives historically wrote physical blocks in 512-byte sectors. An 8,192-byte page consists of **16 sectors** of 512 bytes each ($16 \times 512 = 8192$).
If a power loss occurred mid-write, the page on disk would be left partially updated (*torn page*).

1. **SQL Server Mechanism**:
   Before flushing the page to disk, SQL Server generates a 2-bit torn signature (stored in bits `0..1` of `m_tornBits`, since `m_tornBits` itself resides in Sector 0 at offset 60). For each of the remaining 15 sectors ($s = 1, 2, \dots, 15$, corresponding to byte offsets $1023, 1535, \dots, 8191$), SQL Server saves the original lowest 2 bits of the last byte of sector $s$ into `m_tornBits` at bit position $2s$:
   $$\text{BitSaved}(s) = (m\_tornBits \gg (s \times 2)) \;\&\; 0x03$$
   It then overwrites the lowest 2 bits of the last byte of each sector $s \in [1..15]$ with the 2-bit signature so it can detect partial sector writes on future reads. (Sector 0 at byte 511 is not modified.)
2. **Restoration Algorithm for an Offline Parser**:
   Before parsing records or the Slot Array (since byte 8191 is the high byte of Slot 0's offset!), the engine must restore the original sector bits:
   ```c
   void RestoreTornBits(uint8_t *pageBuffer) {
       uint16_t flagBits = *(uint16_t*)(pageBuffer + 4);
       if (!(flagBits & 0x0100)) return; // Torn bits not enabled

       uint32_t tornBits = *(uint32_t*)(pageBuffer + 60);
       for (int s = 1; s < 16; s++) {
           int sectorEndOffset = (s + 1) * 512 - 1;
           uint8_t origBits = (tornBits >> (s * 2)) & 0x03;
           // Restore the 2 least significant bits of the last byte of sector s
           pageBuffer[sectorEndOffset] = (pageBuffer[sectorEndOffset] & 0xFC) | origBits;
       }
   }
   ```

### 4.2 Page Checksum (Flag 0x0200)
Starting with SQL Server 2005, the default page protection mechanism is Checksum:
1. `m_flagBits & 0x0200 != 0`.
2. The `m_tornBits` field (offset 60) stores the checksum computed across the entire 8,192-byte page.
3. **Validation Algorithm**:
   - Save the original 4-byte value at offset 60.
   - Temporarily zero out the 4 bytes at offset $[60, 63]$.
   - Compute the checksum over the entire 8,192-byte buffer.
   - Compare the computed checksum with the saved value: if they match, the page is 100% intact.
   - Restore the original value at offset 60 before proceeding.

---

## 5. Page Types and Allocation Maps

In the Page Header, the `m_type` field (offset 1) defines the nature of the page:

| `m_type` Value | Constant Name | Functional Description |
|:---:|:---|:---|
| **1** (`0x01`) | `DATA_PAGE` | Data page containing user records in *FixedVar* or compressed *CD* format. |
| **2** (`0x02`) | `INDEX_PAGE` | B-Tree index page (intermediate nodes and leaf nodes of non-clustered indexes). |
| **3** (`0x03`) | `TEXT_PAGE` / `LOB_PAGE` | Page containing large object (LOB) fragments or text trees (TextMix). |
| **4** (`0x04`) | `TEXT_TREE_PAGE` / `SORT_PAGE` | TextTree LOB page or temporary sort page. |
| **7** (`0x07`) | `DIFF_MAP_PAGE` | Differential map page. |
| **8** (`0x08`) | `GAM_PAGE` | Global Allocation Map (tracks allocated/free extents). |
| **9** (`0x09`) | `SGAM_PAGE` | Shared Global Allocation Map (tracks mixed extents with free pages). |
| **10** (`0x0A`) | `IAM_PAGE` | Index Allocation Map (object-specific allocation map). |
| **11** (`0x0B`) | `PFS_PAGE` | Page Free Space (tracks page-by-page allocation and free space). |
| **13** (`0x0D`) | `BOOT_PAGE` | Database boot page (Page 1:9). Contains initial database metadata. |
| **15** (`0x0F`) | `FILE_HEADER_PAGE` | Physical file header page (Page 1:0 or N:0). |
| **16** (`0x10`) | `DCM_PAGE` | Differential Changed Map (extents modified since the last full backup). |
| **17** (`0x11`) | `BCM_PAGE` | Bulk Changed Map (extents modified by bulk-logged operations). |

### 5.1 File Header Page (Type 15 / 0x0F - Page 1:0)
The very first page of every data file (`PageID = 0`) is the **File Header Page**:
- Identifies the file within the database: `m_fileId`, database binding GUID, file size in pages, autogrow settings, minimum and maximum allowed size.
- Contains no user data.

### 5.2 Boot Page (Type 13 / 0x0D - Page 1:9)
The **Boot Page** resides **exclusively at Page 9 of File 1** (`(1:9)`). It is the mandatory entry point for any standalone engine or reader:
- Contains the C++ `dbi` (*Database Info*) structure:
  - `dbi_version`: Internal storage engine version (e.g., 611 = SQL Server 2005, 655 = SQL Server 2008, 706 = SQL Server 2012, 782 = SQL Server 2014, 852 = SQL Server 2016, 895 = SQL Server 2017, 904 = SQL Server 2019, 957 = SQL Server 2022).
  - `dbi_dbname`: Original database name (UTF-16LE Unicode string up to 128 characters).
  - `dbi_dbid`: Database ID assigned by the instance.
  - `dbi_crdate`: Database creation date and time.
  - `Collation`: Default 32-bit database Collation ID (identifies the codepage for `VARCHAR` columns).
  - `dbi_firstSysIndexes`: Page pointer (6 bytes: `PageID` `uint32` + `FileID` `uint16`) pointing to the **first data page of the `sys.sysallocunits` system table** (or `sysindexes`). The entire database bootstrap chain starts from this single pointer!

### 5.3 PFS - Page Free Space (Type 11 / 0x0B)
The PFS page tracks the allocation state and free space percentage for every single page:
- **Placement**: Located at **Page 1** of the file and repeats every **8,088 pages** (pages 1, 8088, 16176, 24264, ...).
- **Coverage**: Each byte in the PFS payload represents 1 data page (covering an interval of 8,088 pages per PFS page).
- **Single PFS Byte Interpretation**:

| Bitmask | Property | Meaning |
|:---:|:---|:---|
| `0x40` | `PFS_ALLOCATED` | **1** = Page is allocated; **0** = Page is free. |
| `0x20` | `PFS_MIXEDEXT` | **1** = Page belongs to a *Mixed Extent*. |
| `0x10` | `PFS_IAM` | **1** = Page is an *Index Allocation Map* (IAM) page. |
| `0x08` | `PFS_GHOST` | **1** = Page contains ghost records awaiting cleanup. |
| `0x07` | `PFS_FULLNESS` | Free space percentage on the page (see below). |

`PFS_FULLNESS` values (`byte & 0x07`):
- `0x00`: Empty page (0% full).
- `0x01`: 1% to 50% full.
- `0x02`: 51% to 80% full.
- `0x03`: 81% to 95% full.
- `0x04`: 96% to 100% full.

### 5.4 GAM and SGAM (Types 8 and 9 / 0x08, 0x09)
Track allocation at the **Extent** level (groups of 8 pages):
- **GAM (Global Allocation Map - Type 8)**:
  - Location: Page 2 of each file (`1:2`) and repeats every **511,232 pages** (~4 GB).
  - Each bit represents an entire extent (8 pages = 64 KB).
  - **Bit = 1**: The extent is **free** and available for allocation.
  - **Bit = 0**: The extent is **allocated** and currently in use.
- **SGAM (Shared Global Allocation Map - Type 9)**:
  - Location: Page 3 of each file (`1:3`) and repeats every **511,232 pages**.
  - Each bit represents an extent.
  - **Bit = 1**: The extent is a **Mixed Extent** and has **at least one free page** available.
  - **Bit = 0**: The extent is either not a mixed extent or all 8 of its pages are currently allocated.

### 5.5 DCM and BCM (Types 16 and 17 / 0x10, 0x11)
- **DCM (Differential Changed Map - Type 16)**:
  - Location: Page 6 (`1:6`) and repeats every 511,232 pages.
  - Bit = 1 indicates that the extent has been modified since the last full backup (used by `BACKUP DATABASE ... WITH DIFFERENTIAL`).
- **BCM (Bulk Changed Map - Type 17)**:
  - Location: Page 7 (`1:7`) and repeats every 511,232 pages.
  - Bit = 1 indicates that the extent has been modified by minimally logged (*bulk-logged*) operations.

### 5.6 IAM - Index Allocation Map (Type 10 / 0x0A)
The **IAM** page is the key structure linking a table or index to its physical pages.
- Each allocation unit has a chain of IAM pages linked via `m_prevPage` and `m_nextPage` pointers.
- **IAM Page Layout**:
  1. **Page Header (96 bytes)**: Standard header.
  2. **IAM Header (within Slot 0 record)**:
     - `sequence_num` (`uint32`): Sequence number of the IAM page in the chain.
     - `status` (`uint32`): Allocation status.
     - `start_pg` (`uint32` PageID + `uint16` FileID): Address of the first page in the covered GAM interval (63,904 extents = 511,232 pages).
     - **Single Page Allocations Array**: Array of **8 page pointers (6 bytes each)**. These hold individual pages allocated from mixed extents during the initial growth of the object.
  3. **IAM Extent Bitmap** (Slot 1, ~7,988 bytes):
     - Contains $7988 \times 8 = 63,904$ bits.
     - Each bit corresponds to one extent in the covered GAM interval.
     - **Bit = 1**: The uniform extent belongs entirely to this allocation unit.
     - **Bit = 0**: The extent does not belong to this allocation unit.

---

## 6. Data Record Format (FixedVar Format)

The default format used by SQL Server to serialize data records on Type 1 pages (and leaf nodes of clustered indexes) is the **FixedVar** format.

### 6.1 General Structure and Record Header (Status Bits A & B)
A data record is structured into sequential sections:

```
+-------------------------------------------------------------------------+
| Status Bits A (1 Byte)                                                  |
+-------------------------------------------------------------------------+
| Status Bits B (1 Byte)                                                  |
+-------------------------------------------------------------------------+
| Fixed-Length Size (2 Bytes, uint16 LE) = pminlen                        |
+-------------------------------------------------------------------------+
| FIXED-LENGTH DATA (pminlen - 4 bytes)                                   |
| Fixed-size columns stored consecutively                                 |
+-------------------------------------------------------------------------+
| Total Column Count (2 Bytes, uint16 LE)                                 |
+-------------------------------------------------------------------------+
| NULL BITMAP (ceil(Total Column Count / 8) Bytes)                        |
| 1 bit per column (1 = NULL, 0 = NOT NULL)                               |
+-------------------------------------------------------------------------+
| [IF BIT 0x20 IS SET IN Status Bits A]:                                  |
|   Variable-Length Column Count (2 Bytes, uint16 LE)                     |
|   Variable-Length Column Offset Array (2 Bytes per varlen column)       |
|   VARIABLE-LENGTH DATA (strings, binaries, LOB pointers...)             |
+-------------------------------------------------------------------------+
```

#### Status Bits A Breakdown (Byte 0 of the record)
- **Bits 1..3 (`0x0E`, `(status_a >> 1) & 0x07`) - Record Type (`RecordType`)**:
  - `0` (`0x00`): **Primary Record** (Standard regular data record).
  - `1` (`0x02`): **Forwarded Record** (Record moved to another page due to size growth in a Heap table; pointed to by a stub).
  - `2` (`0x04`): **Forwarding Stub** (9-byte stub left on the original page holding the 8-byte RID pointer to the new location).
  - `3` (`0x06`): **Index Record** (Record inside a B-Tree index page).
  - `4` (`0x08`): **Blob Fragment** (LOB data fragment or row-overflow chunk).
  - `5` (`0x0A`): **Ghost Index Record** (Logically deleted index record).
  - `6` (`0x0C`): **Ghost Data Record** (Logically deleted data record awaiting ghost cleanup).
- **Bit 4 (`0x10`) - Has Null Bitmap**: Set to **1** in all standard data records.
- **Bit 5 (`0x20`) - Has Variable-Length Columns**:
  - If `1`: The record contains variable-length columns, with their count and offset array immediately following the Null Bitmap.
  - If `0`: The record has no variable-length columns and ends right after the Null Bitmap.
- **Bit 6 (`0x40`) - Versioning Info Present**: Indicates a 14-byte row versioning tag at the end of the record (used in RCSI / Snapshot Isolation).

#### Status Bits B Breakdown (Byte 1 of the record)
- **Bit 0 (`0x01`) - Ghost Forwarded Record**: Indicates that the forwarded record is marked as a ghost record.

### 6.2 Fixed-Length Data Section
- **Offset 2 (2 bytes)**: `FixedLengthSize` (`uint16` Little-Endian).
  - Represents the relative byte offset from the start of the record where the fixed-length portion ends and where `TotalColumnCount` begins.
  - Actual size of fixed-length column payload: $\text{FixedLengthSize} - 4$ bytes.
- Fixed-length columns (`INT`, `BIGINT`, `SMALLINT`, `TINYINT`, `DATETIME`, `FLOAT`, `BINARY(n)`, `CHAR(n)`) are stored sequentially starting at offset 4 of the record.
- **Note**: Even when a fixed-length column is `NULL`, its bytes still occupy their full reserved space in the fixed-length section (its binary bytes are ignored because the corresponding bit in the Null Bitmap is set to 1).

### 6.3 Null Bitmap
Immediately following the fixed-length section (at offset `FixedLengthSize`):
1. **Total Column Count (2 bytes, `uint16` LE)**: Total number of columns tracked in the record ($N$).
2. **Null Bitmap Size**:
   $$\text{NullBitmapBytes} = \lceil N / 8 \rceil = (N + 7) / 8$$
3. **Bit Interpretation**:
   - For column with 0-based nullbit index $k$ ($0 \le k < N$):
     - Byte index: $k / 8$.
     - Bit mask: $1 \ll (k \% 8)$.
     - **Bit = 1**: The column is `NULL`.
     - **Bit = 0**: The column is **NOT NULL** and its value is present in the record.

### 6.4 Variable-Length Section and Offset Array
If bit `0x20` of Status Bits A is set:
1. Immediately after the last byte of the Null Bitmap lies **VarLen Column Count** (2 bytes, `uint16` LE): the number of variable-length columns stored in the record ($V$).
2. **VarLen Column Offset Array**:
   - Immediately follows the count and consists of **$V$ 16-bit integers** (`uint16` LE), totaling $2 \times V$ bytes.
   - Each element stores the **end offset (End Offset)** of that variable-length column's data, relative to the start of the record.
3. **Calculating Start Offset and Length for Variable Column $j$**:
   - Let $\text{EndOffset}(j) = \text{OffsetArray}[j] \;\&\; 0x7FFF$.
   - The start of the data for the first variable-length column ($j=0$) coincides with the end of the offset array:
     $$\text{StartOffset}(0) = \text{VarLenSectionOffset} + 2 + (2 \times V)$$
   - For subsequent columns ($j > 0$):
     $$\text{StartOffset}(j) = \text{OffsetArray}[j - 1] \;\&\; 0x7FFF$$
   - The byte length of column $j$ is:
     $$\text{Length}(j) = \text{EndOffset}(j) - \text{StartOffset}(j)$$

### 6.5 Complex Columns and the 0x8000 Bit Flag
The most significant bit (`0x8000`, high bit flipped when `offset & 0x8000 == 0` in SQL Server's inverted high-bit convention, or masked via `offset & 0x7FFF`) in the variable-length offset array is a **critical flag**:
- The true end offset is always obtained by masking the high bit:
  $$\text{RealEndOffset} = \text{OffsetArray}[j] \;\&\; 0x7FFF$$
- Inside a complex column's payload (starting at `StartOffset`), the first 2 bytes (`uint16` LE) identify the *Complex Type ID*:
  - `2` or `4`: Out-of-row **Blob Inline Root** / Row-Overflow pointer.
  - `5` (or `261`): **Sparse Vector** (storage vector for non-null `SPARSE` columns).
  - `1024` (`0x0400`): **Back Pointer** (reverse pointer to the original forwarding stub in a forwarded record).

### 6.6 Forwarding Stub and Back Pointer (Heap Fragmentation)
In Heap tables (tables without a clustered index), records are referenced by their fixed RID (`PageID:FileID:SlotID`).
When an `UPDATE` increases the size of a variable-length record and the current page lacks sufficient free space:
1. SQL Server moves the entire record to a new page. The record on the new page becomes a **Forwarded Record** (`RecordType = 1`).
2. On the original page, at the original slot, it leaves a **Forwarding Stub** (`RecordType = 2`):
   - Total size: 9 bytes.
   - Byte 0: `Status Bits A = 0x04`.
   - Bytes 1..8: **New 8-byte RID** (`PageID` 4B, `FileID` 2B, `SlotID` 2B) pointing to the new location of the record.
3. The moved record includes a **Back Pointer** (Complex ID 1024) among its trailing variable-length entries storing the original stub RID.
4. **Rule for Offline Parsers**:
   - When performing a point lookup by slot, follow `RecordType == 2` to its target RID.
   - When scanning all data pages of a Heap partition sequentially, process both `PrimaryRecord` (`0`) and `ForwardedRecord` (`1`) directly on their hosting pages and ignore `ForwardingStub` (`2`) so each row is emitted exactly once without redundant cross-page lookups.

### 6.7 Sparse Columns and Sparse Vector
When a table defines columns as `SPARSE`, their values consume zero space in the fixed section or standard variable-length array when `NULL`.
When one or more sparse columns are non-NULL, they are serialized into a single complex column called a **Sparse Vector** (Complex Type ID `5`):
- **Sparse Vector Layout**:
  1. `ComplexType` (2 bytes, `uint16` LE): `0x0005`.
  2. `ColumnCount` (2 bytes, `uint16` LE): Number of non-NULL sparse columns stored ($K$).
  3. `ColumnID Table` ($2 \times K$ bytes): Array of `uint16` LE column IDs sorted in ascending order.
  4. `Offset Table` ($2 \times K$ bytes): Array of `uint16` LE end offsets for each column value within the vector.
  5. `Data Payload`: Serialized binary values of the individual sparse columns.

---

## 7. Data Compression: Row & Page Compression (CD Format)

Starting with SQL Server 2008, tables and indexes can be compressed using **Row Compression** or **Page Compression**.
When a page is compressed, records use the **CD Format (Column Descriptor Format)** instead of *FixedVar*.

### 7.1 CD Record Header and Column Descriptors (4 Bits per Column)
A CD-compressed record consists of:
1. **CD Header (1 byte)**:
   - Bits 1..3: Record type.
   - Bit 4: Page dictionary reference flag.
2. **Column Count (1 or 2 bytes)**:
   - If the most significant bit `0x80` of the first byte is 0: the count is encoded in **1 byte** ($N = \text{byte}$).
   - If bit `0x80` is 1: the count is encoded in **2 bytes Big-Endian**:
     $$N = ((\text{byte}_0 \;\&\; 0x7F) \ll 8) \mid \text{byte}_1$$
3. **Column Descriptors (CD Array)**:
   - Each of the $N$ columns is allocated **4 bits (1 nibble)**.
   - Total descriptor array size: $\lceil N / 2 \rceil$ bytes.
   - The descriptor for column $i$ is in byte $\lfloor i / 2 \rfloor$:
     - Even $i$: low nibble (`byte & 0x0F`).
     - Odd $i$: high nibble (`(byte >> 4) & 0x0F`).

#### Column Descriptor Nibble Values (4-bit Nibble)

| Nibble Value | Meaning | Stored Data Size |
|:---:|:---|:---|
| **0** (`0x0`) | **NULL** | 0 bytes (column is NULL, no data stored). |
| **1** (`0x1`) | **Zero-Byte Short Data** | 0 bytes (numeric zero for integer/float or empty string). |
| **2** (`0x2`) | Short Data - 1 Byte | 1 byte stored in the *Short Data Region*. |
| **3** (`0x3`) | Short Data - 2 Bytes | 2 bytes stored in the *Short Data Region*. |
| **4** (`0x4`) | Short Data - 3 Bytes | 3 bytes stored in the *Short Data Region*. |
| **5** (`0x5`) | Short Data - 4 Bytes | 4 bytes stored in the *Short Data Region*. |
| **6** (`0x6`) | Short Data - 5 Bytes | 5 bytes stored in the *Short Data Region*. |
| **7** (`0x7`) | Short Data - 6 Bytes | 6 bytes stored in the *Short Data Region*. |
| **8** (`0x8`) | Short Data - 7 Bytes | 7 bytes stored in the *Short Data Region*. |
| **9** (`0x9`) | Short Data - 8 Bytes | 8 bytes stored in the *Short Data Region*. |
| **10** (`0xA`) | **Long Data** | Long variable column or LOB stored in the *Long Data Region*. |
| **11** (`0xB`) | **Bit True** | `BIT` column with boolean value **TRUE** (1) (0 bytes consumed!). |
| **12** (`0xC`) | **Page Symbol** | (Page Compression): Value is referenced from the page dictionary. |

### 7.2 Short Data Region and Cluster Array (30 Columns per Cluster)
Values of columns with descriptors 1..9 are grouped in the **Short Data Region**:
- Columns are partitioned into **Clusters of 30 columns each**.
- Immediately after the descriptor array is a cluster offset array storing the starting offset of each 30-column cluster relative to the start of the Short Data Region.
- Within a cluster, the length of each field is $(\text{Descriptor} - 1)$ bytes.

### 7.3 Long Data Region and Offset Array
Columns whose descriptor is `10` (`0x0A` - Long Data) are stored at the end of the record:
- A 2-byte count (`uint16` LE) indicates how many Long columns are present ($L$).
- Followed by an array of $L$ 2-byte offsets (`uint16` LE), analogous to the FixedVar offset array.
- Followed by the long data payloads.

### 7.4 Page Compression: Compression Info (CI), Anchor Record, and Dictionary
**Page Compression** extends Row Compression with **Prefix Compression** and **Dictionary Compression**:
1. **Slot 0** of the Slot Array points to the **Compression Info (CI)** structure at byte 96 (`0x60`):
   - `ci_header` (1 byte): Flags (`0x02` = has Anchor Record, `0x04` = has Dictionary).
   - `ci_modCount` (2 bytes): Page modification counter.
   - `ci_length` (2 bytes): Total length of the CI structure.
2. **Anchor Record**:
   - A CD-formatted record inside CI storing the common prefix for each column across the page.
   - If a row column shares a prefix with the Anchor Record, the row stores only the differing suffix.
3. **Dictionary (Page Dictionary)**:
   - Located inside CI after the Anchor Record.
   - Contains a 2-byte symbol count ($S$), a 2-byte offset array, and deduplicated binary values shared across cells on the page.
   - In user records, a column with CD descriptor `12` (`0x0C` - Page Symbol) stores a 1- or 2-byte index into this page dictionary.

### 7.5 Decoding Compressed Integers: Big-Endian and Inverted Sign Bit
A key reverse-engineering detail documented in OrcaMDF concerns compressed integers (`SMALLINT`, `INT`, `BIGINT`):
- In standard *FixedVar* format, integers are stored in **Little-Endian**.
- In compressed records (*Row/Page Compression*), SQL Server serializes integers in **Big-Endian**, strips leading zero bytes, and **flips the sign bit (XOR with `0x80` on the most significant byte)**!
- **Decompression Formula for a $K$-byte Integer ($1 \le K \le 8$)**:
  ```python
  def decompress_integer(raw_bytes: bytes, signed: bool = True) -> int:
      # raw_bytes has length K (1 to 8 bytes)
      data = bytearray(raw_bytes)
      if signed:
          # Flip the sign bit on the most significant byte (Big-Endian)
          data[0] ^= 0x80
      # Decode Big-Endian
      val = int.from_bytes(data, byteorder="big", signed=False)
      if signed:
          if (raw_bytes[0] & 0x80) == 0:  # Original value was negative
              val = val - (1 << (len(data) * 8))
      return val
  ```

### 7.6 Unicode Compression (SCSU - Standard Compression Scheme for Unicode)
Starting with SQL Server 2008 R2, `NVARCHAR` and `NCHAR` strings under Row/Page Compression can be compressed using **SCSU (Unicode Technical Report #6)**, compressing UTF-16LE text into single-byte sequences using static and dynamic sliding windows.

---

## 8. B-Tree Architecture and Index Records

Tables and indexes in SQL Server are organized as balanced **B+Trees**.

### 8.1 Root, Intermediate, and Leaf Nodes
- The `m_level` field in the Page Header determines the node's level in the B-Tree:
  - `m_level == 0`: **Leaf Level**.
    - In clustered indexes: contains the full user data row with all table columns.
    - In non-clustered indexes: contains the index key columns plus the base row locator (RID or Clustered Key).
  - `m_level > 0`: **Intermediate and Root Nodes (Index Pages)**.
    - Contain index keys and **Down-Page Pointers** to child pages at level `m_level - 1`.

### 8.2 Clustered vs. Non-Clustered Indexes (Heap vs. B-Tree)
- **Heap Table (`IndexID = 0`)**:
  - Unordered collection of pages; allocation is tracked by IAM pages.
- **Clustered Index Table (`IndexID = 1`)**:
  - Leaf pages (`m_level = 0`) hold the actual table rows linked in a doubly linked list (`m_prevPage` and `m_nextPage`) sorted by the clustered key.
- **Non-Clustered Index (`IndexID = 2..250`, or higher)**:
  - Separate B-Tree indexing a subset of columns.

### 8.3 Down-Page Pointer in Intermediate Nodes
In intermediate index nodes (`m_level > 0`, `PageType == 2`):
- Each index record (`RecordType == 3`) contains:
  1. Key column values.
  2. A 6-byte **Down-Page Pointer** at the end of the fixed-length section (`rec[pminlen - 6 .. pminlen]`):
     - `DownPageID` (4 bytes, `uint32` LE).
     - `DownFileID` (2 bytes, `uint16` LE).
- To reach the leftmost leaf page of a B-Tree starting from `pg_root`, descend via Slot 0's Down-Page Pointer until reaching `m_level == 0` (`PageType == 1`), then follow `m_nextPage`.

### 8.4 Uniquifier Handling in Non-Unique Clustered Indexes
If a clustered index is created without the `UNIQUE` constraint:
- SQL Server appends a hidden 4-byte column called **Uniquifier** (`colid = 0`) to guarantee internal uniqueness:
  - For the first occurrence of a key, the Uniquifier is NULL/0 and occupies 0 bytes.
  - For duplicate keys, SQL Server stores a 4-byte `int32` uniquifier in the variable-length section.

### 8.5 RID Pointers vs. Clustered Keys in Non-Clustered Indexes
At the leaf level (`m_level = 0`) of a non-clustered index:
- If the base table is a **Heap**: the index record stores the index key columns plus the **8-byte RID** (`PageID: 4B, FileID: 2B, SlotID: 2B`) pointing to the physical Heap row.
- If the base table has a **Clustered Index**: the index record stores the index key columns plus the **Clustered Index key columns** (including the Uniquifier if non-unique).

---

## 9. Large and Out-of-Row Data Management (LOB & Row-Overflow)

When variable-length values exceed the 8,060-byte in-row limit or use `VARCHAR(MAX)`, `NVARCHAR(MAX)`, `VARBINARY(MAX)`, `TEXT`, `NTEXT`, `IMAGE`, or `XML`, SQL Server stores the payload out-of-row in dedicated **LOB pages** (`PageType::TextMix = 3` and `PageType::TextTree = 4`).

### 9.1 In-Row Pointers: Blob Inline Root (12..72 Bytes) and Text Pointer (16 Bytes)
In the primary data record, a complex pointer replaces the raw bytes:

1. **Legacy Text Pointer (16 Bytes)**:
   - Offset 0..3: `TextTimeStamp` (`uint32`).
   - Offset 4..7: Reserved / rowid.
   - Offset 8..11: `PageID` (`uint32` LE) of the root LOB slot.
   - Offset 12..13: `FileID` (`uint16` LE).
   - Offset 14..15: `SlotID` (`uint16` LE).
2. **Blob Inline Root / Row-Overflow (`ComplexType == 2` or `4`)**:
   - Variable length ($12..72$ bytes).
   - Offset 0..1: `ComplexType` (`2` = BlobInlineRoot, `4` = Row-Overflow).
   - Offset 2: `Type` (`4` = `LARGE_ROOT_SHILOH` / Row-Overflow single pointer; `5` = `LARGE_ROOT_YUKON`).
   - Offset 12+: One or more 12-byte child pointers (`Length/MaxOffset` 4B `uint32` LE, `PageID` 4B `uint32` LE, `FileID` 2B `uint16` LE, `SlotID` 2B `uint16` LE).

### 9.2 LOB Pages (Type 3 / 0x03) and Text Tree Structures
Pages with `m_type == 3` (`TEXT_MIX_PAGE`) or `m_type == 4` (`TEXT_TREE_PAGE`) store LOB fragments organized as a B-Tree of byte streams.

### 9.3 LOB Node Types: SMALL_ROOT, LARGE_ROOT_YUKON, INTERNAL, DATA
Each LOB slot record has a 10-byte outer header (`StatusA`, `StatusB`, `Length` `uint16`, `BlobId` `uint64`) followed by the LOB node payload starting at offset 10 (`lob_type` at `raw[10..12]`):

| Type ID | Constant | Description |
|:---:|:---|:---|
| **0** | `SMALL_ROOT` | Small LOB stored directly inside the root slot (`length` at `raw[12..14]`, raw data at `raw[20 .. 20 + length]`). |
| **1** | `LARGE_ROOT` | Legacy large root containing `count` (`raw[12..14]`) 16-byte child pointers starting at `raw[20]`. |
| **2** | `INTERNAL` | Intermediate B-Tree node containing `count` (`raw[12..14]`) 16-byte child pointers starting at `raw[16]`. |
| **3** | `DATA` | Leaf chunk containing raw payload bytes starting at `raw[10..]`. |
| **5** | `LARGE_ROOT_YUKON` | Yukon large root containing `count` (`raw[14..16]`) 12-byte child pointers starting at `raw[22]`. |

### 9.4 Recursive LOB Stream Reassembly Algorithm
```python
def read_lob_slot(storage, page_id: int, file_id: int, slot_id: int) -> bytes:
    page = storage.get_page(page_id, file_id)
    raw = page.get_slot_slice(slot_id)
    lob_type = int.from_bytes(raw[10:12], "little")

    if lob_type == 3:  # DATA node
        return raw[10:]
    elif lob_type == 0:  # SMALL_ROOT
        length = int.from_bytes(raw[12:14], "little")
        return raw[20 : 20 + length]
    elif lob_type == 2:  # INTERNAL
        count = int.from_bytes(raw[12:14], "little")
        out = bytearray()
        for i in range(count):
            base = 16 + i * 16
            c_page = int.from_bytes(raw[base + 8 : base + 12], "little")
            c_file = int.from_bytes(raw[base + 12 : base + 14], "little")
            c_slot = int.from_bytes(raw[base + 14 : base + 16], "little")
            out.extend(read_lob_slot(storage, c_page, c_file, c_slot))
        return bytes(out)
    elif lob_type == 5:  # LARGE_ROOT_YUKON
        count = int.from_bytes(raw[14:16], "little")
        out = bytearray()
        for i in range(count):
            base = 22 + i * 12
            c_page = int.from_bytes(raw[base + 4 : base + 8], "little")
            c_file = int.from_bytes(raw[base + 8 : base + 10], "little")
            c_slot = int.from_bytes(raw[base + 10 : base + 12], "little")
            out.extend(read_lob_slot(storage, c_page, c_file, c_slot))
        return bytes(out)
```

---

## 10. Binary Encoding and Decoding of All SQL Server Data Types

### 10.1 Integer, Fixed-Point, and Floating-Point Numeric Types

| SQL Type | System Type ID | Physical Bytes | Binary Format and Decoding |
|:---|:---:|:---:|:---|
| `TINYINT` | 48 (`0x30`) | 1 byte | Unsigned 8-bit integer (`uint8`). Range: $0..255$. |
| `SMALLINT` | 52 (`0x34`) | 2 bytes | Signed 16-bit integer (`int16`), Little-Endian. Range: $-32,768..32,767$. |
| `INT` | 56 (`0x38`) | 4 bytes | Signed 32-bit integer (`int32`), Little-Endian. Range: $-2^{31}..2^{31}-1$. |
| `BIGINT` | 127 (`0x7F`) | 8 bytes | Signed 64-bit integer (`int64`), Little-Endian. Range: $-2^{63}..2^{63}-1$. |
| `BIT` | 104 (`0x68`) | Packed bit | Up to 8 `BIT` columns share 1 physical byte in the fixed-length section via `leaf_bitpos`. |
| `FLOAT` | 62 (`0x3E`) | 8 bytes | IEEE 754 Double Precision (64-bit LE). |
| `REAL` | 59 (`0x3B`) | 4 bytes | IEEE 754 Single Precision (32-bit LE). |
| `SMALLMONEY` | 122 (`0x7A`) | 4 bytes | Signed 32-bit LE integer divided by **10,000**. Scale: 4 decimal places. |
| `MONEY` | 60 (`0x3C`) | 8 bytes | Signed 64-bit LE integer divided by **10,000**. |
| `DECIMAL` / `NUMERIC` | 106 / 108 | 5..17 bytes | 1-byte sign (`1` = positive, `0` = negative) + 4/8/12/16-byte unsigned LE integer scaled by $10^{-\text{scale}}$. |

### 10.2 Text Strings and Binary Types (Varchar, Nvarchar, Collation)

| SQL Type | System Type ID | Physical Bytes | Binary Format and Decoding |
|:---|:---:|:---:|:---|
| `CHAR(n)` | 175 (`0xAF`) | $n$ fixed bytes | Single-byte/DBCS string encoded with the column's Collation CodePage. Right-padded with `0x20`. |
| `VARCHAR(n)` | 167 (`0xA7`) | Variable ($0..n$) | Byte sequence encoded with the column's Collation CodePage (or UTF-8 if bit `0x08000000` is set). |
| `NCHAR(n)` | 239 (`0xEF`) | $2 \times n$ fixed bytes | UTF-16LE Unicode string. Right-padded with spaces. |
| `NVARCHAR(n)` | 231 (`0xE7`) | Variable ($0..2n$) | **UTF-16LE** Unicode string. |
| `BINARY(n)` | 173 (`0xAD`) | $n$ fixed bytes | Raw uninterpreted byte sequence. |
| `VARBINARY(n)`| 165 (`0xA5`) | Variable ($0..n$) | Raw uninterpreted byte sequence. |
| `TEXT` / `NTEXT` / `IMAGE` | 35 / 99 / 34 | 16 bytes inline | 16-byte Text Pointer to root LOB slot. |

### 10.3 Dates and Times (DateTime, SmallDateTime, Date, Time, DateTime2, DateTimeOffset)

1. **DATETIME (8 Bytes)**:
   - `Days` (4 bytes, `int32` LE, offset 0..3): Days elapsed since **1900-01-01** (can be negative).
   - `TimeTicks` (4 bytes, `uint32` LE, offset 4..7): Number of $1/300$-second ticks since midnight ($\approx 3.333\text{ ms}$).
2. **SMALLDATETIME (4 Bytes)**:
   - `Days` (2 bytes, `uint16` LE, offset 0..1): Days since **1900-01-01**.
   - `Minutes` (2 bytes, `uint16` LE, offset 2..3): Minutes since midnight ($0..1439$).
3. **DATE (3 Bytes)**:
   - 24-bit unsigned Little-Endian integer representing days elapsed since **`0001-01-01`**.
4. **TIME (3..5 Bytes)**:
   - Size depends on fractional scale $S \in [0..7]$: $S \in [0..2] \to 3\text{ B}$, $S \in [3..4] \to 4\text{ B}$, $S \in [5..7] \to 5\text{ B}$.
   - Little-Endian integer storing time intervals of $10^{-S}$ seconds since midnight.
5. **DATETIME2 (6..8 Bytes)**:
   - Binary concatenation of `TIME` ($3..5$ bytes) followed by `DATE` (last 3 bytes).
6. **DATETIMEOFFSET (8..10 Bytes)**:
   - `DATETIME2` ($6..8$ bytes, UTC) + `OffsetMinutes` (2 bytes, signed `int16` LE).

### 10.4 UniqueIdentifier (GUID)
- Size: **16 bytes** using Microsoft mixed-endian layout:
  - `Data1`: 4 bytes (`uint32` LE).
  - `Data2`: 2 bytes (`uint16` LE).
  - `Data3`: 2 bytes (`uint16` LE).
  - `Data4`: 8 bytes (Big-Endian raw byte array).
- Canonical format: `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`.

### 10.5 SqlVariant
- Byte 0: Base `TypeID` (`uint8`).
- Byte 1: `Version` (`uint8`).
- Followed by type-specific property bytes (collation, precision/scale, max length) and the serialized value payload.

### 10.6 Complex and Spatial Types (CLR UDT, HierarchyId, Geometry, Geography, XML)
- **XML**: Stored in-row or as LOB; may be UTF-16LE text or **SQL Server Binary XML (`0xDF 0xFF`)**.
- **HierarchyId / Geometry / Geography**: Serialized CLR UDT binary streams.

---

## 11. System Catalog Bootstrapping (System Base Tables)

### 11.1 Initialization Chain (Boot Page -> sysallocunits)

```
[File 1, Page 9: BOOT PAGE]
          │
          ▼ Reads dbi_firstSysIndexes (PageID:FileID)
[sys.sysallocunits (Object ID 7 / AUID 0x0001000000070000)]
          │
          ├────────────────────────┬────────────────────────┐
          ▼                        ▼                        ▼
[sys.sysrowsets (5)]       [sys.sysrscols (3)]      [sys.sysschobjs (34)]
  Partitions & Rowset IDs   Physical column layout   Tables ('U '), FKs ('F '),
  (idmajor / idminor)       (offset, nullbit, colid) Views & Schema Objects
```

1. **Open Primary File**: Open the `.mdf` file (`FileID = 1`).
2. **Read Boot Page**: Read physical page 9 at byte offset $9 \times 8192 = 73,728$.
3. **Extract `dbi_firstSysIndexes`**: Points to the root/first page of **`sys.sysallocunits`** (`idmajor = 7`).
4. **Scan `sys.sysallocunits`**: Locate the allocation units of `sys.sysrowsets` (`idmajor = 5`), then join `sys.sysrowsets` with `sys.sysallocunits` (`auid == rowsetid | (1 << 48)`) to find all other system base tables!

### 11.2 Core System Base Tables
1. **`sys.sysallocunits` (`idmajor = 7`)**:
   - Links each allocation unit (`auid`, `type`, `ownerid`) to its physical pages (`pgfirst`, `pgroot`, `pgfirstiam`).
2. **`sys.sysrowsets` (`idmajor = 5`)**:
   - Maps logical rowsets (`rowsetid`, `idmajor = object_id`, `idminor = index_id`).
3. **`sys.sysrscols` (`idmajor = 3`)**:
   - Describes the physical layout of every column within a rowset:
     - `rsid` (`0..8`): Rowset ID.
     - `rscolid` (`8..12`): Logical column ID (joins with `syscolpars.colid`).
     - `hbcolid` (`12..16`): Physical storage ordinal.
     - `offset` (`28..30`): Physical fixed offset (`> 0`) or 1-based variable-length index (`< 0`).
     - `nullbit` (`30..32`): 1-based index in the Null Bitmap.
     - `bitpos` (`32`): Bit index (`0..7`) for `BIT` columns.
4. **`sys.sysschobjs` (`idmajor = 34`)**:
   - Contains database schema objects: user tables (`type == "U "`), foreign key constraints (`type == "F "`), views (`type == "V "`), etc.
   - For `"F "` foreign keys: `id` is `fk_object_id`, `pid` (`15..19`) is `parent_object_id`, `(status >> 12) & 3` is `ON DELETE` action, and `(status >> 14) & 3` is `ON UPDATE` action (`0 = NO ACTION`, `1 = CASCADE`, `2 = SET NULL`, `3 = SET DEFAULT`).
5. **`sys.syscolpars` (`idmajor = 41`)**:
   - Maps logical table columns: `id` (`object_id`), `colid`, `xtype` (System Type ID), `length`, `prec`, `scale`, `collationid`, `status`, `name` (`UTF-16LE`).
6. **`sys.sysowners` (`idmajor = 44`)**:
   - Maps `nsid` (`schema_id`) to schema names (`dbo`, etc.).
7. **`sys.sysidxstats` (`idmajor = 54`) & `sys.siscols` (`idmajor = 55`)**:
   - `sysidxstats`: Index definitions (`id`, `indid`, `status` where `0x08` = `UNIQUE` and `0x20` = `PRIMARY KEY`, `type` where `1` = Clustered and `2` = Non-Clustered, `name`).
   - `siscols`: Index key columns (`idmajor = object_id`, `idminor = indid`, `subid = key_ordinal`, `status` where `0x01` = key column and `0x04` = `DESC`, `intprop = colid`, `tinyprop1`).
8. **`sys.syssingleobjrefs` (`idmajor = 74`) & `sys.sysmultiobjrefs` (`idmajor = 75`)**:
   - Store object/column references (21-byte fixed record: `class: u8`, `depid: i32`, `depsubid: i32`, `indepid: i32`, `indepsubid: i32`, `status: i32`).
   - For Foreign Keys (`depid == fk_object_id`):
     - `class == 27`: `indepid` is `referenced_object_id`.
     - `class == 28`: `depsubid` is `constraint_column_id`, `indepsubid` is `parent_column_id`.
     - `class == 29`: `depsubid` is `constraint_column_id`, `indepsubid` is `referenced_column_id`.

### 11.3 Complete Schema Resolution Algorithm
1. Read `sys.sysschobjs` (`type == "U "`) and `sys.sysowners` to discover all user tables and schemas.
2. Read `sys.syscolpars` (`number == 0`) to obtain logical columns, types, lengths, precision/scale, and collations.
3. Read `sys.sysrowsets` (`idminor <= 1`) and `sys.sysallocunits` (`type == 1`) to locate the physical partitions of each table, and join `sys.sysrscols` (`rscolid == colid`) to obtain the physical `leaf_offset`, `leaf_nullbit`, and `leaf_bitpos` of every column.
4. Read `sys.sysidxstats` and `sys.siscols` to reconstruct all Clustered and Non-Clustered indexes.
5. Read `sys.sysschobjs` (`type == "F "`), `sys.syssingleobjrefs`, and `sys.sysmultiobjrefs` to reconstruct all Foreign Key constraints.
6. Traverse B-Tree leaf chains (`idminor == 1`) or IAM extent allocations (`idminor == 0`) to extract and decode all rows.

---

## 12. Multi-File Database Management (.MDF + .NDF) and Filegroups

### 12.1 Filegroup Architecture and Proportional Fill Allocation
- **Filegroup**: Logical grouping of physical files (`PRIMARY` contains `.mdf`; secondary filegroups may hold `.ndf` files).
- **Proportional Fill**: SQL Server distributes new extents across files in a filegroup proportionally to their free space.

### 12.2 Cross-File Pointer Resolution
Every page pointer is a pair $\langle \text{PageID (4 bytes)}, \; \text{FileID (2 bytes)} \rangle$. A multi-file parser maps each `FileID` to its respective memory-mapped file handle and computes $\text{ByteOffset} = \text{PageID} \times 8192$.

---

## 13. Related Formats: Transaction Log (.LDF) and Backups (.BAK / MTF)

### 13.1 Transaction Log (.LDF) and VLF Architecture
- Divided into sequential **Virtual Log Files (VLFs)** containing log records identified by **LSN (`VLF_ID : Offset : Slot_ID`)** with `TransactionID`, `Operation`, `Context`, and Redo/Undo byte images.

### 13.2 Microsoft Tape Format Backup (.BAK / MTF)
- Native SQL Server `.bak` files encapsulate MDF/NDF pages and log streams inside **Microsoft Tape Format (MTF)** descriptor blocks (`TAPE`, `VOLB`, `SSET`, `FILE`, `DATA`, `SPAD`).

---

## 14. Conversion Patterns to Other Engines (e.g., SQLite)

### 14.1 SQL Server to SQLite Type Mapping

| SQL Server Source Type | SQLite Target Affinity | Transformation and Conversion Rule |
|:---|:---|:---|
| `BIT`, `TINYINT`, `SMALLINT`, `INT`, `BIGINT` | `INTEGER` | Direct mapping to 64-bit signed SQLite integer. |
| `FLOAT`, `REAL` | `REAL` | Direct mapping to 64-bit IEEE 754 floating-point. |
| `MONEY`, `SMALLMONEY`, `DECIMAL`, `NUMERIC` | `NUMERIC` / `TEXT` | Exact decimal string representation to preserve financial precision without floating-point rounding. |
| `CHAR`, `VARCHAR`, `NCHAR`, `NVARCHAR`, `TEXT`, `NTEXT`, `XML` | `TEXT` | Decoded from UTF-16LE or Windows Collation CodePage into valid **UTF-8**. |
| `BINARY`, `VARBINARY`, `IMAGE`, `ROWVERSION` | `BLOB` | Raw uninterpreted byte stream. |
| `UNIQUEIDENTIFIER` | `TEXT` | Canonical 36-character GUID string: `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`. |
| `DATETIME`, `DATETIME2`, `SMALLDATETIME`, `DATE`, `TIME`, `DATETIMEOFFSET` | `TEXT` | Formatted as ISO-8601 strings (`YYYY-MM-DD HH:MM:SS.fff`). |

### 14.2 Constraints, Trigger-Based Foreign Keys, and Indexes
- **Foreign Keys**: Embedded directly in `CREATE TABLE` DDL (`CONSTRAINT "..." FOREIGN KEY (...) REFERENCES "..." (...) [ON DELETE ...] [ON UPDATE ...]`).
- **Indexes**: Created via `CREATE UNIQUE INDEX` / `CREATE INDEX` preserving key column ordering and `ASC`/`DESC` sort direction.

### 14.3 High-Performance Bulk Insertion Strategies
- **WAL Mode with Incremental Commits**: Configure `PRAGMA journal_mode = WAL; PRAGMA locking_mode = NORMAL; PRAGMA synchronous = NORMAL;`, commit all `CREATE TABLE` statements upfront, and commit data insertions in batches (e.g. every 50,000 rows and per table) so concurrent readers can query the `.sqlite3` database while it is being populated.

---

## 15. Implementation Guide for a Cross-Platform Parser (Linux & Windows)

### 15.1 Layered Parser Architecture

```
+-------------------------------------------------------------------------+
| LAYER 4: Schema & Migration Engine (catalog.rs, sqlite.rs, main.rs)     |
|   System catalog bootstrap (sysschobjs, sysrscols, sysidxstats, FKs),   |
|   parallel WAL-mode export to SQLite3 with indexes and foreign keys.    |
+-------------------------------------------------------------------------+
| LAYER 3: Record, LOB & Type Decoder (record.rs, lob.rs, types.rs)       |
|   FixedVar record parser, Sparse Vector, LOB tree reassembly,           |
|   Windows codepage & UTF-16LE -> UTF-8 conversion.                      |
+-------------------------------------------------------------------------+
| LAYER 2: Page & Allocation Engine (page.rs)                             |
|   96B Page Header parser, Torn Page sector restoration (1..15),         |
|   PFS allocation checks, IAM extent scanning, B-Tree traversal.         |
+-------------------------------------------------------------------------+
| LAYER 1: Physical File I/O Layer (memmap2)                              |
|   Cross-platform read-only memory mapping (mmap on Linux,               |
|   CreateFileMapping/MapViewOfFile on Windows) indexed in 8192B pages.   |
+-------------------------------------------------------------------------+
```

### 15.2 High-Performance I/O: Memory Mapping vs. Paged Streaming
- **Memory-Mapped Files (`memmap2`)**: Uses `mmap` on Linux and `CreateFileMapping` / `MapViewOfFile` on Windows for zero-copy read-only random page access (`&mmap[page_id * 8192 .. (page_id + 1) * 8192]`).

### 15.3 Endianness Handling and Packed Structures
- All multi-byte fields in SQL Server pages are **Little-Endian** (`u16::from_le_bytes`, `u32::from_le_bytes`, `i64::from_le_bytes`).

### 15.4 Compliance Checklist and Integrity Tests
- [x] **Test 1: Page Header & Boot Page Verification**: Open `Page 1:0` and `Page 1:9`, verify `m_headerVersion == 1`, and parse `dbi_version`, `dbi_dbname`, `dbi_collation`, and `dbi_firstSysIndexes`.
- [x] **Test 2: Sector De-tornification**: Restore the lowest 2 bits of sectors `1..15` (`bytes 1023..8191`) when `m_flagBits & 0x0100 != 0`.
- [x] **Test 3: System Metadata Bootstrap**: Execute `Boot Page (1:9) -> sysallocunits -> sysrowsets -> sysrscols -> sysschobjs -> syscolpars -> sysidxstats -> siscols -> syssingleobjrefs` offline.
- [x] **Test 4: Heap Table Extraction**: Scan IAM single-page slots and extent bitmaps verified via PFS allocation bits (`0x40`).
- [x] **Test 5: Clustered Index B-Tree Traversal**: Descend from `pg_root` (`m_level > 0`) to leaf pages (`m_level == 0`) and follow `m_nextPage`.
- [x] **Test 6: Out-of-Row LOB Reassembly**: Reconstruct `TEXT`, `NTEXT`, `IMAGE`, `VARCHAR(MAX)`, `NVARCHAR(MAX)`, `VARBINARY(MAX)`, and `XML` trees (`SMALL_ROOT`, `INTERNAL`, `DATA`, `LARGE_ROOT_YUKON`).
- [x] **Test 7: Full UTF-8 Conversion**: Decode both `UTF-16LE` and Windows Collation codepages into valid UTF-8.
- [x] **Test 8: Automatic Index & Foreign Key Migration in WAL Mode**: Export all tables, foreign keys, data rows, and indexes into SQLite3 in `WAL` mode.
