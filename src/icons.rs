use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use pelite::PeFile;
use pelite::resources::FindError;

const MANAGED_ICON_SIZE: u32 = 256;
const ICO_HEADER_LEN: usize = 6;
const ICO_DIR_ENTRY_LEN: usize = 16;
const MAX_ICON_DIR_ENTRIES: usize = 64;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PNG_IEND: &[u8; 4] = b"IEND";
const PE_SIGNATURE_OFFSET: usize = 0x3c;
const PE_FILE_HEADER_LEN: usize = 20;
const PE_SECTION_HEADER_LEN: usize = 40;
const PE_DATA_DIRECTORY_LEN: usize = 8;
const PE_RESOURCE_DIRECTORY_INDEX: usize = 2;
const RESOURCE_DIRECTORY_TABLE_LEN: usize = 16;
const RESOURCE_DIRECTORY_ENTRY_LEN: usize = 8;
const RESOURCE_DATA_ENTRY_LEN: usize = 16;
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const MAX_GROUP_ICON_ENTRIES: usize = 256;

pub fn game_icon_path(game_id: &str) -> PathBuf {
    managed_icons_dir_path().join(format!("{}.png", game_icon_name(game_id)))
}

pub fn group_icon_path(group_id: &str) -> PathBuf {
    managed_icons_dir_path().join(format!("{}.png", group_icon_name(group_id)))
}

pub fn game_icon_name(game_id: &str) -> String {
    format!("{}.game-{}", crate::APP_ID, game_id)
}

pub fn group_icon_name(group_id: &str) -> String {
    format!("{}.group-{}", crate::APP_ID, group_id)
}

pub fn game_icon_file(game_id: &str) -> Option<PathBuf> {
    let path = game_icon_path(game_id);
    path.is_file().then_some(path)
}

pub fn group_icon_file(group_id: &str) -> Option<PathBuf> {
    let path = group_icon_path(group_id);
    path.is_file().then_some(path)
}

pub fn extract_game_icon(game_id: &str, exe_path: &str) -> Result<(), String> {
    let exe_path = Path::new(exe_path.trim());
    if exe_path.as_os_str().is_empty() {
        return Err("Executable path is required for icon extraction".to_string());
    }

    let target = game_icon_path(game_id);
    ensure_icons_dir()?;
    extract_best_icon_to_png(exe_path, &target, MANAGED_ICON_SIZE)
}

pub fn save_custom_game_icon(game_id: &str, source: &str) -> Result<(), String> {
    let target = game_icon_path(game_id);
    ensure_icons_dir()?;
    save_custom_icon(Path::new(source.trim()), &target)
}

pub fn save_custom_group_icon(group_id: &str, source: &str) -> Result<(), String> {
    let target = group_icon_path(group_id);
    ensure_icons_dir()?;
    save_custom_icon(Path::new(source.trim()), &target)
}

pub fn clear_game_icon(game_id: &str) {
    let _ = fs::remove_file(game_icon_path(game_id));
}

pub fn clear_group_icon(group_id: &str) {
    let _ = fs::remove_file(group_icon_path(group_id));
}

fn save_custom_icon(source: &Path, target: &Path) -> Result<(), String> {
    if source.as_os_str().is_empty() {
        return Err("Custom icon path is required".to_string());
    }

    let image = image::open(source)
        .map_err(|err| format!("Failed to read custom icon '{}': {}", source.display(), err))?;
    let normalized = image.resize(MANAGED_ICON_SIZE, MANAGED_ICON_SIZE, FilterType::CatmullRom);

    normalized
        .save_with_format(target, image::ImageFormat::Png)
        .map_err(|err| {
            format!(
                "Failed to write managed icon '{}': {}",
                target.display(),
                err
            )
        })
}

fn ensure_icons_dir() -> Result<PathBuf, String> {
    let path = managed_icons_dir_path();
    fs::create_dir_all(&path).map_err(|err| {
        format!(
            "Failed to create managed icon directory '{}': {}",
            path.display(),
            err
        )
    })?;
    Ok(path)
}

fn managed_icons_dir_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/share/icons/hicolor/256x256/apps")
}

fn extract_best_icon_to_png(exe_path: &Path, out: &Path, size: u32) -> Result<(), String> {
    let bytes = fs::read(exe_path)
        .map_err(|err| format!("Failed to read '{}': {}", exe_path.display(), err))?;

    let decoded = find_best_group_icon(&bytes, size)
        .or_else(|| find_best_png_icon(&bytes, size))
        .or_else(|| find_ico_blob(&bytes).and_then(decode_icon_blob))
        .ok_or_else(|| format!("No icon resource found in '{}'", exe_path.display()))?;

    decoded
        .resize(size, size, FilterType::CatmullRom)
        .save_with_format(out, image::ImageFormat::Png)
        .map_err(|err| format!("Failed to save icon '{}': {}", out.display(), err))
}

fn decode_icon_blob(bytes: &[u8]) -> Option<image::DynamicImage> {
    image::load_from_memory(bytes).ok()
}

fn score_match(image: &image::DynamicImage, target_size: u32) -> u64 {
    let target = u64::from(target_size);
    u64::from(image.width()).abs_diff(target) + u64::from(image.height()).abs_diff(target)
}

#[derive(Clone, Copy)]
struct SectionHeader {
    virtual_address: u32,
    virtual_size: u32,
    raw_ptr: u32,
    raw_size: u32,
}

#[derive(Clone)]
struct GroupIconEntry {
    width: u8,
    height: u8,
    color_count: u8,
    reserved: u8,
    planes: u16,
    bit_count: u16,
    icon_id: u16,
}

fn find_best_group_icon(bytes: &[u8], size: u32) -> Option<image::DynamicImage> {
    find_best_group_icon_pelite(bytes, size).or_else(|| find_best_group_icon_manual(bytes, size))
}

fn find_best_group_icon_pelite(bytes: &[u8], size: u32) -> Option<image::DynamicImage> {
    let pe = PeFile::from_bytes(bytes).ok()?;
    let resources = pe.resources().ok()?;
    let mut best: Option<(u64, image::DynamicImage)> = None;

    for icon_result in resources.icons() {
        let (_name, group_icon) = match icon_result {
            Ok(icon) => icon,
            Err(FindError::NotFound) => continue,
            Err(_) => continue,
        };

        let mut ico = Vec::new();
        if group_icon.write(&mut ico).is_err() {
            continue;
        }
        let Some(decoded) = decode_icon_blob(&ico) else {
            continue;
        };

        let score = score_match(&decoded, size);
        if best.as_ref().is_none_or(|(current, _)| score < *current) {
            best = Some((score, decoded));
        }
    }

    best.map(|(_, decoded)| decoded)
}

fn find_best_group_icon_manual(bytes: &[u8], size: u32) -> Option<image::DynamicImage> {
    let pe_offset = usize::try_from(read_u32_le(bytes, PE_SIGNATURE_OFFSET)?).ok()?;
    if bytes.get(pe_offset..pe_offset + 4)? != b"PE\0\0" {
        return None;
    }

    let coff_offset = pe_offset + 4;
    let number_of_sections = usize::from(read_u16_le(bytes, coff_offset + 2)?);
    let optional_header_size = usize::from(read_u16_le(bytes, coff_offset + 16)?);
    let optional_header_offset = coff_offset + PE_FILE_HEADER_LEN;
    let sections_offset = optional_header_offset + optional_header_size;

    let section_headers = read_section_headers(bytes, sections_offset, number_of_sections)?;
    let resource_directory = read_resource_directory(bytes, optional_header_offset)?;
    let resource_root = rva_to_file_offset(resource_directory.0, &section_headers)?;

    let icon_blobs =
        collect_icon_resource_blobs(bytes, resource_root, resource_directory.1, &section_headers)?;
    let groups =
        collect_group_icon_blobs(bytes, resource_root, resource_directory.1, &section_headers)?;

    let mut best: Option<(u64, image::DynamicImage)> = None;
    for group_blob in groups {
        let Some(ico_blob) = build_ico_from_group(group_blob, &icon_blobs) else {
            continue;
        };
        let Some(decoded) = decode_icon_blob(&ico_blob) else {
            continue;
        };

        let score = score_match(&decoded, size);
        if best.as_ref().is_none_or(|(current, _)| score < *current) {
            best = Some((score, decoded));
        }
    }

    best.map(|(_, image)| image)
}

fn read_resource_directory(bytes: &[u8], optional_header_offset: usize) -> Option<(u32, u32)> {
    let magic = read_u16_le(bytes, optional_header_offset)?;
    let data_directory_offset = if magic == 0x010b {
        optional_header_offset + 96
    } else if magic == 0x020b {
        optional_header_offset + 112
    } else {
        return None;
    };

    let entry_offset =
        data_directory_offset + (PE_RESOURCE_DIRECTORY_INDEX * PE_DATA_DIRECTORY_LEN);
    let rva = read_u32_le(bytes, entry_offset)?;
    let size = read_u32_le(bytes, entry_offset + 4)?;
    if rva == 0 || size == 0 {
        return None;
    }

    Some((rva, size))
}

fn read_section_headers(
    bytes: &[u8],
    mut offset: usize,
    number_of_sections: usize,
) -> Option<Vec<SectionHeader>> {
    let mut sections = Vec::with_capacity(number_of_sections);
    for _ in 0..number_of_sections {
        if offset.checked_add(PE_SECTION_HEADER_LEN)? > bytes.len() {
            return None;
        }
        let virtual_size = read_u32_le(bytes, offset + 8)?;
        let virtual_address = read_u32_le(bytes, offset + 12)?;
        let raw_size = read_u32_le(bytes, offset + 16)?;
        let raw_ptr = read_u32_le(bytes, offset + 20)?;
        sections.push(SectionHeader {
            virtual_address,
            virtual_size,
            raw_ptr,
            raw_size,
        });
        offset += PE_SECTION_HEADER_LEN;
    }
    Some(sections)
}

fn collect_icon_resource_blobs<'a>(
    bytes: &'a [u8],
    resource_root: usize,
    resource_size: u32,
    sections: &[SectionHeader],
) -> Option<BTreeMap<u16, &'a [u8]>> {
    let mut icons = BTreeMap::new();
    for data in collect_resource_data_for_type(bytes, resource_root, resource_size, RT_ICON)? {
        let icon_id = u16::try_from(data.id).ok()?;
        let blob = read_resource_data_entry(bytes, data.data_offset, sections)?;
        icons.insert(icon_id, blob);
    }
    Some(icons)
}

fn collect_group_icon_blobs<'a>(
    bytes: &'a [u8],
    resource_root: usize,
    resource_size: u32,
    sections: &[SectionHeader],
) -> Option<Vec<&'a [u8]>> {
    let mut groups = Vec::new();
    for data in collect_resource_data_for_type(bytes, resource_root, resource_size, RT_GROUP_ICON)?
    {
        let blob = read_resource_data_entry(bytes, data.data_offset, sections)?;
        groups.push(blob);
    }
    Some(groups)
}

#[derive(Clone, Copy)]
struct ResourceLeaf {
    id: u32,
    data_offset: usize,
}

fn collect_resource_data_for_type(
    bytes: &[u8],
    root_offset: usize,
    resource_size: u32,
    target_type: u32,
) -> Option<Vec<ResourceLeaf>> {
    let type_entries = read_resource_entries(bytes, root_offset)?;
    let type_entry = type_entries
        .into_iter()
        .find(|entry| !entry.name_is_string && entry.id == target_type)?;
    let type_dir = resolve_resource_subdir(root_offset, type_entry.offset_to_data, resource_size)?;
    let name_entries = read_resource_entries(bytes, type_dir)?;
    let mut leaves = Vec::new();

    for name_entry in name_entries {
        collect_resource_leaves_from_entry(
            bytes,
            root_offset,
            resource_size,
            name_entry.id,
            name_entry.offset_to_data,
            0,
            &mut leaves,
        )?;
    }

    Some(leaves)
}

fn collect_resource_leaves_from_entry(
    bytes: &[u8],
    root_offset: usize,
    resource_size: u32,
    name_id: u32,
    offset_to_data: u32,
    depth: usize,
    out: &mut Vec<ResourceLeaf>,
) -> Option<()> {
    if depth > 2 {
        return Some(());
    }

    if (offset_to_data & 0x8000_0000) == 0 {
        let data_offset = resolve_resource_data_entry(root_offset, offset_to_data, resource_size)?;
        out.push(ResourceLeaf {
            id: name_id,
            data_offset,
        });
        return Some(());
    }

    let dir_offset = resolve_resource_subdir(root_offset, offset_to_data, resource_size)?;
    let entries = read_resource_entries(bytes, dir_offset)?;
    for entry in entries {
        if (entry.offset_to_data & 0x8000_0000) == 0 {
            let data_offset =
                resolve_resource_data_entry(root_offset, entry.offset_to_data, resource_size)?;
            out.push(ResourceLeaf {
                id: name_id,
                data_offset,
            });
        } else {
            collect_resource_leaves_from_entry(
                bytes,
                root_offset,
                resource_size,
                name_id,
                entry.offset_to_data,
                depth + 1,
                out,
            )?;
        }
    }

    Some(())
}

#[derive(Clone, Copy)]
struct ResourceDirectoryEntry {
    id: u32,
    name_is_string: bool,
    offset_to_data: u32,
}

fn read_resource_entries(bytes: &[u8], dir_offset: usize) -> Option<Vec<ResourceDirectoryEntry>> {
    if dir_offset.checked_add(RESOURCE_DIRECTORY_TABLE_LEN)? > bytes.len() {
        return None;
    }
    let named_count = usize::from(read_u16_le(bytes, dir_offset + 12)?);
    let id_count = usize::from(read_u16_le(bytes, dir_offset + 14)?);
    let count = named_count.checked_add(id_count)?;
    let mut entries = Vec::with_capacity(count);
    let mut offset = dir_offset + RESOURCE_DIRECTORY_TABLE_LEN;
    for _ in 0..count {
        if offset.checked_add(RESOURCE_DIRECTORY_ENTRY_LEN)? > bytes.len() {
            return None;
        }
        let name_raw = read_u32_le(bytes, offset)?;
        let data_raw = read_u32_le(bytes, offset + 4)?;
        entries.push(ResourceDirectoryEntry {
            id: name_raw & 0x7fff_ffff,
            name_is_string: (name_raw & 0x8000_0000) != 0,
            offset_to_data: data_raw,
        });
        offset += RESOURCE_DIRECTORY_ENTRY_LEN;
    }
    Some(entries)
}

fn resolve_resource_subdir(
    root_offset: usize,
    raw_offset: u32,
    resource_size: u32,
) -> Option<usize> {
    if (raw_offset & 0x8000_0000) == 0 {
        return None;
    }
    let relative = usize::try_from(raw_offset & 0x7fff_ffff).ok()?;
    let absolute = root_offset.checked_add(relative)?;
    let max = root_offset.checked_add(usize::try_from(resource_size).ok()?)?;
    if absolute >= max {
        return None;
    }
    Some(absolute)
}

fn resolve_resource_data_entry(
    root_offset: usize,
    raw_offset: u32,
    resource_size: u32,
) -> Option<usize> {
    if (raw_offset & 0x8000_0000) != 0 {
        return None;
    }
    let relative = usize::try_from(raw_offset).ok()?;
    let absolute = root_offset.checked_add(relative)?;
    let max = root_offset.checked_add(usize::try_from(resource_size).ok()?)?;
    let end = absolute.checked_add(RESOURCE_DATA_ENTRY_LEN)?;
    if end > max {
        return None;
    }
    Some(absolute)
}

fn read_resource_data_entry<'a>(
    bytes: &'a [u8],
    data_entry_offset: usize,
    sections: &[SectionHeader],
) -> Option<&'a [u8]> {
    let data_rva = read_u32_le(bytes, data_entry_offset)?;
    let data_size = usize::try_from(read_u32_le(bytes, data_entry_offset + 4)?).ok()?;
    let data_offset = rva_to_file_offset(data_rva, sections)?;
    bytes.get(data_offset..data_offset.checked_add(data_size)?)
}

fn rva_to_file_offset(rva: u32, sections: &[SectionHeader]) -> Option<usize> {
    for section in sections {
        let size = section.virtual_size.max(section.raw_size);
        let start = section.virtual_address;
        let end = start.checked_add(size)?;
        if (start..end).contains(&rva) {
            let within = rva.checked_sub(start)?;
            let file_offset = section.raw_ptr.checked_add(within)?;
            return usize::try_from(file_offset).ok();
        }
    }
    None
}

fn build_ico_from_group(group_blob: &[u8], icons_by_id: &BTreeMap<u16, &[u8]>) -> Option<Vec<u8>> {
    if read_u16_le(group_blob, 0)? != 0 || read_u16_le(group_blob, 2)? != 1 {
        return None;
    }

    let count = usize::from(read_u16_le(group_blob, 4)?);
    if count == 0 || count > MAX_GROUP_ICON_ENTRIES {
        return None;
    }
    let entries_size = count.checked_mul(14)?;
    if 6_usize.checked_add(entries_size)? > group_blob.len() {
        return None;
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let offset = 6 + (i * 14);
        entries.push(GroupIconEntry {
            width: *group_blob.get(offset)?,
            height: *group_blob.get(offset + 1)?,
            color_count: *group_blob.get(offset + 2)?,
            reserved: *group_blob.get(offset + 3)?,
            planes: read_u16_le(group_blob, offset + 4)?,
            bit_count: read_u16_le(group_blob, offset + 6)?,
            icon_id: read_u16_le(group_blob, offset + 12)?,
        });
    }

    let mut images = Vec::new();
    for entry in &entries {
        let icon = icons_by_id.get(&entry.icon_id)?;
        images.push(*icon);
    }

    let mut output = Vec::new();
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&(u16::try_from(count).ok()?).to_le_bytes());

    let mut image_offset = 6 + (count * 16);
    for (entry, image) in entries.iter().zip(&images) {
        output.push(entry.width);
        output.push(entry.height);
        output.push(entry.color_count);
        output.push(entry.reserved);
        output.extend_from_slice(&entry.planes.to_le_bytes());
        output.extend_from_slice(&entry.bit_count.to_le_bytes());
        output.extend_from_slice(&(u32::try_from(image.len()).ok()?).to_le_bytes());
        output.extend_from_slice(&(u32::try_from(image_offset).ok()?).to_le_bytes());
        image_offset = image_offset.checked_add(image.len())?;
    }

    for image in images {
        output.extend_from_slice(image);
    }
    Some(output)
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn find_best_png_icon(bytes: &[u8], size: u32) -> Option<image::DynamicImage> {
    let mut best_match: Option<(u64, image::DynamicImage)> = None;
    let mut cursor = 0_usize;

    while let Some(start) = find_bytes(bytes, PNG_SIGNATURE, cursor) {
        if let Some(end) = parse_png_end(bytes, start)
            && let Some(blob) = bytes.get(start..end)
            && let Some(decoded) = decode_icon_blob(blob)
        {
            let score = score_match(&decoded, size);
            if best_match
                .as_ref()
                .is_none_or(|(current_score, _)| score < *current_score)
            {
                best_match = Some((score, decoded));
            }
            cursor = end;
            continue;
        }
        cursor = start.saturating_add(1);
    }

    best_match.map(|(_, decoded)| decoded)
}

fn parse_png_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut offset = start + PNG_SIGNATURE.len();

    while offset.checked_add(12)? <= bytes.len() {
        let length = u32::from_be_bytes([
            *bytes.get(offset)?,
            *bytes.get(offset + 1)?,
            *bytes.get(offset + 2)?,
            *bytes.get(offset + 3)?,
        ]) as usize;
        let chunk_type_offset = offset + 4;
        let chunk_data_offset = chunk_type_offset + 4;
        let chunk_end = chunk_data_offset.checked_add(length)?.checked_add(4)?;

        if chunk_end > bytes.len() {
            return None;
        }

        let chunk_type = bytes.get(chunk_type_offset..chunk_type_offset + 4)?;
        offset = chunk_end;

        if chunk_type == PNG_IEND {
            return Some(chunk_end);
        }
    }

    None
}

fn find_bytes(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() || needle.len() > haystack.len() {
        return None;
    }

    let end = haystack.len() - needle.len();
    (start..=end).find(|&idx| haystack[idx..idx + needle.len()] == *needle)
}

fn find_ico_blob(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < ICO_HEADER_LEN {
        return None;
    }

    for index in 0..=(bytes.len() - ICO_HEADER_LEN) {
        if bytes[index] != 0
            || bytes[index + 1] != 0
            || bytes[index + 2] != 1
            || bytes[index + 3] != 0
        {
            continue;
        }

        let entry_count = usize::from(u16::from_le_bytes([bytes[index + 4], bytes[index + 5]]));
        if entry_count == 0 || entry_count > MAX_ICON_DIR_ENTRIES {
            continue;
        }

        let table_len = ICO_HEADER_LEN + (entry_count * ICO_DIR_ENTRY_LEN);
        if index
            .checked_add(table_len)
            .is_none_or(|end| end > bytes.len())
        {
            continue;
        }

        let mut end_offset = index + table_len;
        let mut valid_entries = true;

        for entry in 0..entry_count {
            let entry_offset = index + ICO_HEADER_LEN + (entry * ICO_DIR_ENTRY_LEN);
            let image_size = u32::from_le_bytes([
                bytes[entry_offset + 8],
                bytes[entry_offset + 9],
                bytes[entry_offset + 10],
                bytes[entry_offset + 11],
            ]) as usize;
            let image_offset = u32::from_le_bytes([
                bytes[entry_offset + 12],
                bytes[entry_offset + 13],
                bytes[entry_offset + 14],
                bytes[entry_offset + 15],
            ]) as usize;

            let absolute_offset = index + image_offset;
            let image_end = absolute_offset.saturating_add(image_size);
            if image_size == 0
                || image_offset < table_len
                || absolute_offset < index
                || image_end > bytes.len()
            {
                valid_entries = false;
                break;
            }
            end_offset = end_offset.max(image_end);
        }

        if valid_entries {
            return bytes.get(index..end_offset);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        build_ico_from_group, decode_icon_blob, find_best_png_icon, find_ico_blob, game_icon_path,
        group_icon_path,
    };
    use image::{ImageBuffer, Rgba};
    use std::collections::BTreeMap;

    #[test]
    fn finds_embedded_ico_blob() {
        let bytes = [
            1_u8, 2, 3, 4, 0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0, 4, 0, 0, 0, 22, 0, 0, 0,
            0xaa, 0xbb, 0xcc, 0xdd,
        ];

        let found = find_ico_blob(&bytes);
        assert!(found.is_some());
    }

    #[test]
    fn rejects_broken_entry_offsets() {
        let bytes = [
            0_u8, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0, 4, 0, 0, 0, 250, 0, 0, 0,
        ];
        assert!(find_ico_blob(&bytes).is_none());
    }

    #[test]
    fn extracts_embedded_png_blob() {
        let image = ImageBuffer::from_pixel(2, 2, Rgba([255_u8, 0, 0, 255]));
        let mut png_bytes = Vec::new();
        let encoded = image::DynamicImage::ImageRgba8(image).write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        );
        assert!(encoded.is_ok());

        let mut payload = vec![1_u8, 2, 3, 4];
        payload.extend_from_slice(&png_bytes);
        payload.extend_from_slice(&[9_u8, 8, 7, 6]);

        let decoded = find_best_png_icon(&payload, 2);
        assert!(decoded.is_some());
        let decoded = decoded.unwrap_or_else(|| image::DynamicImage::new_rgba8(1, 1));
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
    }

    #[test]
    fn rebuilds_single_entry_group_icon_from_twenty_byte_header() {
        let icon_payload = vec![1_u8, 2, 3, 4];
        let mut icons = BTreeMap::new();
        icons.insert(1_u16, icon_payload.clone());

        let mut group = Vec::new();
        group.extend_from_slice(&0_u16.to_le_bytes());
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.push(16);
        group.push(16);
        group.push(0);
        group.push(0);
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.extend_from_slice(&32_u16.to_le_bytes());
        group.extend_from_slice(&(u32::try_from(icon_payload.len()).unwrap_or(0)).to_le_bytes());
        group.extend_from_slice(&1_u16.to_le_bytes());

        let icons = icons
            .iter()
            .map(|(id, payload)| (*id, payload.as_slice()))
            .collect();
        let rebuilt = build_ico_from_group(&group, &icons);
        assert!(rebuilt.is_some());
        let rebuilt = rebuilt.unwrap_or_default();

        assert_eq!(&rebuilt[0..2], &0_u16.to_le_bytes());
        assert_eq!(&rebuilt[2..4], &1_u16.to_le_bytes());
        assert_eq!(&rebuilt[4..6], &1_u16.to_le_bytes());
        assert_eq!(&rebuilt[22..26], &icon_payload);
    }

    #[test]
    fn decodes_png_backed_single_entry_group_icon() {
        let image = ImageBuffer::from_pixel(2, 2, Rgba([255_u8, 0, 0, 255]));
        let mut png_bytes = Vec::new();
        let encoded = image::DynamicImage::ImageRgba8(image).write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        );
        assert!(encoded.is_ok());

        let mut icons = BTreeMap::new();
        icons.insert(1_u16, png_bytes.clone());

        let mut group = Vec::new();
        group.extend_from_slice(&0_u16.to_le_bytes());
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.push(2);
        group.push(2);
        group.push(0);
        group.push(0);
        group.extend_from_slice(&1_u16.to_le_bytes());
        group.extend_from_slice(&32_u16.to_le_bytes());
        group.extend_from_slice(&(u32::try_from(png_bytes.len()).unwrap_or(0)).to_le_bytes());
        group.extend_from_slice(&1_u16.to_le_bytes());

        let icons = icons
            .iter()
            .map(|(id, payload)| (*id, payload.as_slice()))
            .collect();
        let rebuilt = build_ico_from_group(&group, &icons);
        assert!(rebuilt.is_some());
        let decoded = decode_icon_blob(&rebuilt.unwrap_or_default());
        assert!(decoded.is_some());
    }

    #[test]
    fn managed_icon_paths_use_hicolor_app_directory() {
        let game_path = game_icon_path("game-1");
        let group_path = group_icon_path("group-1");
        let game_rendered = game_path.to_string_lossy();
        let group_rendered = group_path.to_string_lossy();

        assert!(game_rendered.contains(".local/share/icons/hicolor/256x256/apps"));
        assert!(group_rendered.contains(".local/share/icons/hicolor/256x256/apps"));
        assert!(game_rendered.ends_with("com.github.sachesi.leyen.game-game-1.png"));
        assert!(group_rendered.ends_with("com.github.sachesi.leyen.group-group-1.png"));
    }
}
