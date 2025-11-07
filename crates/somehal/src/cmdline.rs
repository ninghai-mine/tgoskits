#[unsafe(link_section = ".data")]
static CMDLINE: [u8; 4096] = [0; 4096];

pub fn set_cmdline(cmdline: &str) {
    let bytes = cmdline.as_bytes();
    let len = bytes.len().min(CMDLINE.len() - 1);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), CMDLINE.as_ptr() as *mut u8, len);
        // Null-terminate
        (CMDLINE.as_ptr() as *mut u8).add(len).write(0);
    }
}
pub fn cmdline() -> Option<&'static str> {
    if CMDLINE[0] == 0 {
        return None;
    }
    let len = CMDLINE
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(CMDLINE.len());
    Some(unsafe { core::str::from_utf8_unchecked(&CMDLINE[..len]) })
}

pub fn var(key: &str) -> Option<&'static str> {
    let cmdline = cmdline()?;
    for pair in cmdline.split_whitespace() {
        if let Some(pos) = pair.find('=') {
            let (k, v) = pair.split_at(pos);
            if k == key {
                return Some(&v[1..]);
            }
        }
    }
    None
}
