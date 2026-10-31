// Shared by the build scripts of the app and of the setup program (through `include!`): makes the
// compiled resource file that holds the icon.

/// A compiled resource file holding the .ico's images as icon resources, and their directory as
/// icon group 1, which Explorer shows for the exe and the window loads at run time.
fn icon_res(ico: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    let count = u16_at(4);
    let mut out = Vec::new();
    // every .res starts with an empty entry
    entry(&mut out, 0, 0, 0, &[]);
    let mut group = vec![0, 0, 1, 0];
    group.extend(count.to_le_bytes());
    for i in 0..count {
        let at = 6 + 16 * usize::from(i);
        let (size, offset) = (u32_at(at + 8), u32_at(at + 12));
        entry(&mut out, RT_ICON, i + 1, 0x1010, &ico[offset..offset + size]);
        // the directory entry as in the .ico, with the resource's number in place of its offset
        group.extend(&ico[at..at + 12]);
        group.extend((i + 1).to_le_bytes());
    }
    entry(&mut out, RT_GROUP_ICON, 1, 0x1030, &group);
    out
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// One resource: its header (numbered type and name, neutral language) and data, padded to 4 bytes.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(name.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend([0u8; 8]);
    out.extend(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

const RT_VERSION: u16 = 16;

/// Version information as resource 1: what Explorer's Details tab, Task Manager and Windows' app
/// lists show for the exe, such as "Flores PDF" rather than its file name.
fn version_entry(out: &mut Vec<u8>, description: &str, file_name: &str) {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let mut fixed = Vec::new();
    for v in [0xFEEF_04BD, 0x0001_0000, major << 16 | minor, patch << 16, major << 16 | minor, patch << 16, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0u32] {
        fixed.extend(v.to_le_bytes());
    }
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", "aga2522"),
        ("FileDescription", description),
        ("FileVersion", version.as_str()),
        ("InternalName", file_name),
        ("OriginalFilename", file_name),
        ("ProductName", "Flores PDF"),
        ("ProductVersion", version.as_str()),
    ]
    .iter()
    .map(|(k, v)| {
        let text: Vec<u8> = v.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        node(k, &text, (text.len() / 2) as u16, 1, &[])
    })
    .collect();
    let table = node("040904B0", &[], 0, 1, &strings);
    let string_info = node("StringFileInfo", &[], 0, 1, &[table]);
    // US English, Unicode
    let translation: Vec<u8> = [0x0409u16, 0x04B0].iter().flat_map(|v| v.to_le_bytes()).collect();
    let var = node("Translation", &translation, 4, 0, &[]);
    let var_info = node("VarFileInfo", &[], 0, 1, &[var]);
    let info = node("VS_VERSION_INFO", &fixed, fixed.len() as u16, 0, &[string_info, var_info]);
    entry(out, RT_VERSION, 1, 0x0030, &info);
}

/// One node of version information: its length, value's length and type, its key, then its value
/// and children, each starting on 4 bytes.
fn node(key: &str, value: &[u8], value_len: u16, kind: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut n = vec![0, 0];
    n.extend(value_len.to_le_bytes());
    n.extend(kind.to_le_bytes());
    n.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    n.resize(n.len().next_multiple_of(4), 0);
    n.extend(value);
    for child in children {
        n.resize(n.len().next_multiple_of(4), 0);
        n.extend(child);
    }
    let len = n.len() as u16;
    n[..2].copy_from_slice(&len.to_le_bytes());
    n
}
