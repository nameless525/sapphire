//! A Windows executable's resources, read from its bytes (M4 S4): the application manifest, VERSIONINFO and
//! the icons, as `cargo xtask package --platform windows` checks them, with the subsystem it runs under.
//!
//! The PE format is Microsoft's ("PE Format", learn.microsoft.com), and VERSIONINFO's nodes are
//! `VS_VERSIONINFO`'s ("VS_VERSIONINFO structure", the same site). Only what the check reads is read: the
//! headers' resource directory and sections, the resource tree's three levels (type, name, language), and
//! VERSIONINFO's fixed numbers and string table. Anything malformed is an error, never a panic.

use std::collections::BTreeMap;

use anyhow::{Context as _, bail, ensure};

/// `RT_ICON`: one image of an icon.
pub const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`: an icon, as a directory of its images.
pub const RT_GROUP_ICON: u16 = 14;
/// `RT_VERSION`: VERSIONINFO.
pub const RT_VERSION: u16 = 16;
/// `RT_MANIFEST`: the application manifest. The loader reads ID 1 at process creation.
pub const RT_MANIFEST: u16 = 24;
/// `IMAGE_SUBSYSTEM_WINDOWS_GUI`: a program with no console.
pub const SUBSYSTEM_GUI: u16 = 2;

/// One resource: its type and name (`None` for a name that's a string rather than an ID) and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub kind: u16,
    pub id: Option<u16>,
    pub language: u16,
    pub data: Vec<u8>,
}

/// What an executable says about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct Executable {
    /// `IMAGE_SUBSYSTEM_*` from the optional header.
    pub subsystem: u16,
    pub resources: Vec<Resource>,
}

impl Executable {
    /// Reads an executable's headers and its resource tree.
    pub fn read(bytes: &[u8]) -> anyhow::Result<Self> {
        let file = Bytes(bytes);
        ensure!(bytes.starts_with(b"MZ"), "not a Windows executable (no MZ header)");
        let pe = usize::try_from(file.u32(0x3c)?)?;
        ensure!(file.slice(pe, 4)? == b"PE\0\0", "no PE signature at {pe:#x}");
        let coff = pe + 4;
        let sections = usize::from(file.u16(coff + 2)?);
        let optional_size = usize::from(file.u16(coff + 16)?);
        let optional = coff + 20;
        let (count_at, directories_at) = match file.u16(optional)? {
            0x10b => (optional + 92, optional + 96),
            0x20b => (optional + 108, optional + 112),
            magic => bail!("an optional header of unknown magic {magic:#x}"),
        };
        let subsystem = file.u16(optional + 68)?;
        let directories = file.u32(count_at)?;
        let mut sections_list = Vec::with_capacity(sections);
        let table = optional + optional_size;
        for index in 0..sections {
            let header = table + index * 40;
            sections_list.push(Section {
                virtual_size: file.u32(header + 8)?,
                address: file.u32(header + 12)?,
                raw_size: file.u32(header + 16)?,
                raw_offset: file.u32(header + 20)?,
            });
        }
        // The resource table is the third data directory.
        let (rva, size) = if directories > 2 {
            (file.u32(directories_at + 16)?, file.u32(directories_at + 20)?)
        } else {
            (0, 0)
        };
        let resources = if rva == 0 || size == 0 {
            Vec::new()
        } else {
            let image = Image { file, sections: sections_list };
            let root = image.offset(rva).context("the resource table's address is in no section")?;
            image.resources(root)?
        };
        Ok(Self { subsystem, resources })
    }

    /// Every resource of `kind`.
    pub fn of_kind(&self, kind: u16) -> impl Iterator<Item = &Resource> {
        self.resources.iter().filter(move |resource| resource.kind == kind)
    }
}

#[derive(Clone, Copy, Debug)]
struct Section {
    virtual_size: u32,
    address: u32,
    raw_size: u32,
    raw_offset: u32,
}

struct Image<'a> {
    file: Bytes<'a>,
    sections: Vec<Section>,
}

impl Image<'_> {
    /// The file offset of a relative virtual address, if a section's data holds it.
    fn offset(&self, rva: u32) -> Option<usize> {
        self.sections.iter().find_map(|section| {
            let within = rva.checked_sub(section.address)?;
            (within < section.virtual_size.max(section.raw_size) && within < section.raw_size)
                .then(|| usize::try_from(section.raw_offset.checked_add(within)?).ok())
                .flatten()
        })
    }

    /// The resource tree under the directory at file offset `root`: types, then names, then languages, each
    /// leaf a data entry whose address is relative to the image.
    fn resources(&self, root: usize) -> anyhow::Result<Vec<Resource>> {
        let mut resources = Vec::new();
        for (kind, types) in self.directory(root, root)? {
            let (Some(kind), Entry::Directory(names)) = (kind, types) else { continue };
            for (id, names) in self.directory(root, names)? {
                let Entry::Directory(languages) = names else {
                    bail!("resource type {kind} has a bare entry")
                };
                for (language, leaf) in self.directory(root, languages)? {
                    let Entry::Data(entry) = leaf else { bail!("resource type {kind} nests too deep") };
                    let data_rva = self.file.u32(entry)?;
                    let size = usize::try_from(self.file.u32(entry + 4)?)?;
                    let at = self
                        .offset(data_rva)
                        .with_context(|| format!("resource type {kind}'s data is in no section"))?;
                    let data = self.file.slice(at, size)?.to_vec();
                    resources.push(Resource { kind, id, language: language.unwrap_or(0), data });
                }
            }
        }
        Ok(resources)
    }

    /// A directory's entries, each with its ID (or `None` for a string name), at file offset `at`; the
    /// entries' offsets are from the tree's root. The tree is read three levels deep and no further, so a
    /// directory that points back can't loop.
    fn directory(&self, root: usize, at: usize) -> anyhow::Result<Vec<(Option<u16>, Entry)>> {
        let named = usize::from(self.file.u16(at + 12)?);
        let ids = usize::from(self.file.u16(at + 14)?);
        let mut entries = Vec::with_capacity(named + ids);
        for index in 0..named + ids {
            let entry = at + 16 + index * 8;
            let name = self.file.u32(entry)?;
            let target = self.file.u32(entry + 4)?;
            let id = (name & 0x8000_0000 == 0).then(|| (name & 0xffff) as u16);
            let offset = root + usize::try_from(target & 0x7fff_ffff)?;
            let child =
                if target & 0x8000_0000 != 0 { Entry::Directory(offset) } else { Entry::Data(offset) };
            entries.push((id, child));
        }
        Ok(entries)
    }
}

enum Entry {
    Directory(usize),
    Data(usize),
}

/// VERSIONINFO: the fixed numbers, and the first string table's values.
#[derive(Debug, PartialEq, Eq)]
pub struct VersionInfo {
    pub file_version: [u16; 4],
    pub product_version: [u16; 4],
    /// The string table's language and code page, as its key: `040904B0` for US English in Unicode.
    pub table: String,
    pub strings: BTreeMap<String, String>,
}

impl VersionInfo {
    /// Reads a `VS_VERSIONINFO` resource.
    pub fn read(data: &[u8]) -> anyhow::Result<Self> {
        let bytes = Bytes(data);
        let root = Node::read(bytes, 0)?;
        ensure!(root.key == "VS_VERSION_INFO", "VERSIONINFO's root is {:?}", root.key);
        let fixed = Bytes(root.value);
        ensure!(fixed.0.len() >= 52, "VERSIONINFO has no fixed file info");
        ensure!(fixed.u32(0)? == 0xfeef_04bd, "VS_FIXEDFILEINFO's signature is wrong");
        let version = |at: usize| -> anyhow::Result<[u16; 4]> {
            let (high, low) = (fixed.u32(at)?, fixed.u32(at + 4)?);
            Ok([(high >> 16) as u16, high as u16, (low >> 16) as u16, low as u16])
        };
        let (file_version, product_version) = (version(8)?, version(16)?);

        let mut table = None;
        for child in root.children(bytes)? {
            if child.key != "StringFileInfo" {
                continue;
            }
            let Some(strings) = child.children(bytes)?.into_iter().next() else { continue };
            let mut values = BTreeMap::new();
            for string in strings.children(bytes)? {
                values.insert(string.key.clone(), string.text()?);
            }
            table = Some((strings.key, values));
            break;
        }
        let (table, strings) = table.context("VERSIONINFO has no string table")?;
        Ok(Self { file_version, product_version, table, strings })
    }
}

/// One node of VERSIONINFO's tree: a length, a value's length and kind, a UTF-16 key, the value and the
/// children, each part aligned to 4 bytes from the resource's start.
struct Node<'a> {
    start: usize,
    end: usize,
    key: String,
    value: &'a [u8],
    children_at: usize,
}

impl<'a> Node<'a> {
    fn read(bytes: Bytes<'a>, start: usize) -> anyhow::Result<Self> {
        let length = usize::from(bytes.u16(start)?);
        let value_length = usize::from(bytes.u16(start + 2)?);
        let text = bytes.u16(start + 4)? == 1;
        ensure!(length >= 6, "a VERSIONINFO node of {length} bytes");
        let end = start + length;
        ensure!(end <= bytes.0.len(), "a VERSIONINFO node runs past the resource");
        let (key, after_key) = bytes.utf16_z(start + 6, end)?;
        let value_at = align4(after_key);
        // A string's length counts characters, and compilers differ on whether it counts the NUL: a string
        // runs to its NUL or its node's end, read by `text`.
        let value_bytes =
            if text { (end.saturating_sub(value_at)).min(value_length * 2) } else { value_length };
        let value_end = (value_at + value_bytes).min(end);
        let value = if value_at <= value_end { &bytes.0[value_at..value_end] } else { &[][..] };
        let children_at = align4(value_end);
        Ok(Self { start, end, key, value, children_at })
    }

    fn children(&self, bytes: Bytes<'a>) -> anyhow::Result<Vec<Node<'a>>> {
        let mut children = Vec::new();
        let mut at = self.children_at;
        while at + 6 <= self.end {
            let child = Node::read(bytes, at)?;
            ensure!(child.end <= self.end, "a VERSIONINFO node at {:#x} overruns its parent", child.start);
            at = align4(child.end);
            children.push(child);
        }
        Ok(children)
    }

    /// A string node's value, to its NUL.
    fn text(&self) -> anyhow::Result<String> {
        let units: Vec<u16> = self
            .value
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|&unit| unit != 0)
            .collect();
        String::from_utf16(&units).with_context(|| format!("{}'s value isn't UTF-16", self.key))
    }
}

fn align4(at: usize) -> usize {
    at.next_multiple_of(4)
}

/// Bounds-checked little-endian reads.
#[derive(Clone, Copy)]
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn slice(self, at: usize, len: usize) -> anyhow::Result<&'a [u8]> {
        at.checked_add(len).and_then(|end| self.0.get(at..end)).context("the file is cut short")
    }

    fn u16(self, at: usize) -> anyhow::Result<u16> {
        Ok(u16::from_le_bytes(self.slice(at, 2)?.try_into()?))
    }

    fn u32(self, at: usize) -> anyhow::Result<u32> {
        Ok(u32::from_le_bytes(self.slice(at, 4)?.try_into()?))
    }

    /// A NUL-terminated UTF-16 string from `at`, before `end`, and where its NUL ends.
    fn utf16_z(self, at: usize, end: usize) -> anyhow::Result<(String, usize)> {
        let mut units = Vec::new();
        let mut cursor = at;
        loop {
            ensure!(cursor + 2 <= end, "a VERSIONINFO key runs past its node");
            let unit = self.u16(cursor)?;
            cursor += 2;
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        Ok((String::from_utf16(&units).context("a VERSIONINFO key isn't UTF-16")?, cursor))
    }
}

/// Writers of what [`Executable::read`] and [`VersionInfo::read`] read, laid out as Microsoft documents them,
/// for tests here and in the package's checks.
#[cfg(test)]
pub mod build {
    use super::*;

    /// A PE32+ executable for x86-64 with the given subsystem and, in one `.rsrc` section, the resources.
    pub fn executable(subsystem: u16, resources: &[Resource]) -> Vec<u8> {
        const RSRC_RVA: u32 = 0x1000;
        const FILE_ALIGN: usize = 0x200;
        let rsrc = resource_section(resources, RSRC_RVA);

        let pe: usize = 0x80;
        let optional_size: usize = 240;
        let headers = pe + 4 + 20 + optional_size + 40;
        let raw_offset = headers.next_multiple_of(FILE_ALIGN);
        let mut out = vec![0u8; raw_offset];
        out[..2].copy_from_slice(b"MZ");
        put32(&mut out, 0x3c, pe as u32);
        out[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        put16(&mut out, coff, 0x8664);
        put16(&mut out, coff + 2, 1);
        put16(&mut out, coff + 16, optional_size as u16);
        put16(&mut out, coff + 18, 0x22);
        let optional = coff + 20;
        put16(&mut out, optional, 0x20b);
        put16(&mut out, optional + 68, subsystem);
        put32(&mut out, optional + 108, 16);
        put32(&mut out, optional + 112 + 16, RSRC_RVA);
        put32(&mut out, optional + 112 + 20, rsrc.len() as u32);
        let section = optional + optional_size;
        out[section..section + 5].copy_from_slice(b".rsrc");
        put32(&mut out, section + 8, rsrc.len() as u32);
        put32(&mut out, section + 12, RSRC_RVA);
        let raw_size = rsrc.len().next_multiple_of(FILE_ALIGN);
        put32(&mut out, section + 16, raw_size as u32);
        put32(&mut out, section + 20, raw_offset as u32);
        out.extend_from_slice(&rsrc);
        out.resize(raw_offset + raw_size, 0);
        out
    }

    /// A resource section at `rva`: the tree (types, then IDs, then languages, each sorted as the format
    /// asks), its data entries, then the data, each aligned to 4.
    fn resource_section(resources: &[Resource], rva: u32) -> Vec<u8> {
        let mut by_kind: BTreeMap<u16, BTreeMap<u16, Vec<&Resource>>> = BTreeMap::new();
        for resource in resources {
            let id = resource.id.expect("tests name resources by ID");
            by_kind.entry(resource.kind).or_default().entry(id).or_default().push(resource);
        }
        let directory = |entries: usize| 16 + entries * 8;
        // Lay out the directories first, then the data entries, then the data.
        let mut size = directory(by_kind.len());
        for ids in by_kind.values() {
            size += directory(ids.len());
            for languages in ids.values() {
                size += directory(languages.len());
            }
        }
        let entries_at = size;
        let data_at = entries_at + resources.len() * 16;
        let mut out = vec![0u8; data_at];
        let (mut next_directory, mut next_entry) = (directory(by_kind.len()), entries_at);
        put16(&mut out, 14, by_kind.len() as u16);
        for (type_index, (kind, ids)) in by_kind.iter().enumerate() {
            let ids_at = next_directory;
            next_directory += directory(ids.len());
            put32(&mut out, 16 + type_index * 8, u32::from(*kind));
            put32(&mut out, 16 + type_index * 8 + 4, 0x8000_0000 | ids_at as u32);
            put16(&mut out, ids_at + 14, ids.len() as u16);
            for (id_index, (id, languages)) in ids.iter().enumerate() {
                let languages_at = next_directory;
                next_directory += directory(languages.len());
                put32(&mut out, ids_at + 16 + id_index * 8, u32::from(*id));
                put32(&mut out, ids_at + 16 + id_index * 8 + 4, 0x8000_0000 | languages_at as u32);
                put16(&mut out, languages_at + 14, languages.len() as u16);
                for (language_index, resource) in languages.iter().enumerate() {
                    let entry = next_entry;
                    next_entry += 16;
                    put32(&mut out, languages_at + 16 + language_index * 8, u32::from(resource.language));
                    put32(&mut out, languages_at + 16 + language_index * 8 + 4, entry as u32);
                    let data = out.len().next_multiple_of(4);
                    out.resize(data, 0);
                    out.extend_from_slice(&resource.data);
                    put32(&mut out, entry, rva + data as u32);
                    put32(&mut out, entry + 4, resource.data.len() as u32);
                }
            }
        }
        out
    }

    /// `VS_VERSIONINFO` as rc.exe writes it: the fixed numbers, a string table under `table`, and the
    /// translation.
    pub fn version_info(file: [u16; 4], product: [u16; 4], table: &str, strings: &[(&str, &str)]) -> Vec<u8> {
        let ms_ls = |v: [u16; 4]| {
            [(u32::from(v[0]) << 16) | u32::from(v[1]), (u32::from(v[2]) << 16) | u32::from(v[3])]
        };
        let mut fixed = Vec::new();
        let [file_ms, file_ls] = ms_ls(file);
        let [product_ms, product_ls] = ms_ls(product);
        for word in
            [0xfeef_04bd, 0x0001_0000, file_ms, file_ls, product_ms, product_ls, 0x3f, 0, 0x40004, 1, 0, 0, 0]
        {
            fixed.extend_from_slice(&u32::to_le_bytes(word));
        }
        let strings: Vec<Vec<u8>> = strings
            .iter()
            .map(|(key, value)| {
                let text: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
                node(key, (value.encode_utf16().count() + 1) as u16, true, &text, &[])
            })
            .collect();
        let table = node(table, 0, true, &[], &strings);
        let string_info = node("StringFileInfo", 0, true, &[], &[table]);
        let translation = node("Translation", 4, false, &[0x09, 0x04, 0xb0, 0x04], &[]);
        let var_info = node("VarFileInfo", 0, true, &[], &[translation]);
        node("VS_VERSION_INFO", fixed.len() as u16, false, &fixed, &[string_info, var_info])
    }

    fn node(key: &str, value_length: u16, text: bool, value: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0u8; 6];
        put16(&mut out, 2, value_length);
        put16(&mut out, 4, u16::from(text));
        out.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        out.resize(out.len().next_multiple_of(4), 0);
        out.extend_from_slice(value);
        for child in children {
            out.resize(out.len().next_multiple_of(4), 0);
            out.extend_from_slice(child);
        }
        let length = out.len() as u16;
        put16(&mut out, 0, length);
        out
    }

    fn put16(out: &mut [u8], at: usize, value: u16) {
        out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(out: &mut [u8], at: usize, value: u32) {
        out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(kind: u16, id: u16, data: &[u8]) -> Resource {
        Resource { kind, id: Some(id), language: 0x409, data: data.to_vec() }
    }

    #[test]
    fn reads_resources_and_the_subsystem() {
        let manifest = b"<assembly/>";
        let resources = [
            resource(RT_ICON, 1, &[1, 2, 3]),
            resource(RT_ICON, 2, &[4, 5, 6, 7, 8]),
            resource(RT_GROUP_ICON, 1, &[9; 34]),
            resource(RT_MANIFEST, 1, manifest),
        ];
        let read = Executable::read(&build::executable(SUBSYSTEM_GUI, &resources)).unwrap();
        assert_eq!(read.subsystem, SUBSYSTEM_GUI);
        assert_eq!(read.resources, resources);
        assert_eq!(read.of_kind(RT_ICON).count(), 2);
        assert_eq!(read.of_kind(RT_MANIFEST).next().unwrap().data, manifest);

        let none = Executable::read(&build::executable(3, &[])).unwrap();
        assert_eq!((none.subsystem, none.resources.len()), (3, 0));
    }

    #[test]
    fn reads_versioninfo() {
        let data = build::version_info(
            [0, 1, 0, 1188],
            [0, 1, 0, 1188],
            "040904B0",
            &[("FileVersion", "0.1.0.1188"), ("ProductName", "Sapphire"), ("ProductVersion", "0.1.0+1188")],
        );
        let info = VersionInfo::read(&data).unwrap();
        assert_eq!(info.file_version, [0, 1, 0, 1188]);
        assert_eq!(info.product_version, [0, 1, 0, 1188]);
        assert_eq!(info.table, "040904B0");
        assert_eq!(info.strings["ProductName"], "Sapphire");
        assert_eq!(info.strings["ProductVersion"], "0.1.0+1188");
        assert_eq!(info.strings.len(), 3);

        // Inside an executable, as the check finds it.
        let exe = build::executable(SUBSYSTEM_GUI, &[resource(RT_VERSION, 1, &data)]);
        let read = Executable::read(&exe).unwrap();
        assert_eq!(VersionInfo::read(&read.of_kind(RT_VERSION).next().unwrap().data).unwrap(), info);
    }

    /// Bytes from outside are an error when they're wrong, never a panic.
    #[test]
    fn refuses_what_isnt_an_executable() {
        assert!(Executable::read(b"\x7fELF").unwrap_err().to_string().contains("no MZ"));
        let exe = build::executable(SUBSYSTEM_GUI, &[resource(RT_MANIFEST, 1, b"<assembly/>")]);
        for cut in [0x40, 0x90, 0x150, 0x220] {
            assert!(Executable::read(&exe[..cut]).is_err(), "cut at {cut:#x}");
        }
        let mut wrong = exe.clone();
        wrong[0x80] = b'X';
        assert!(Executable::read(&wrong).unwrap_err().to_string().contains("no PE signature"));

        let data = build::version_info([1, 2, 3, 4], [1, 2, 3, 4], "040904B0", &[("A", "b")]);
        for cut in [4, 20, 60, data.len() - 4] {
            assert!(VersionInfo::read(&data[..cut]).is_err(), "cut at {cut}");
        }
        let mut signature = data.clone();
        signature[40] ^= 0xff;
        assert!(VersionInfo::read(&signature).unwrap_err().to_string().contains("signature"));
    }
}
