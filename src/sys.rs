//! Réglages système pour la fluidité (Windows uniquement ; sans effet ailleurs).

/// À appeler une fois au démarrage.
/// - Priorité élevée pour le processus.
/// - Minuteur système à 1 ms (par défaut ~15 ms), pour des attentes et un son plus réguliers.
pub fn boost_process() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Media::timeBeginPeriod,
            System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS},
        };
        SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
        timeBeginPeriod(1);
    }
}

/// Donne une priorité légèrement supérieure au thread courant (capture, affichage).
pub fn boost_thread() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
        };
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

/// Nom commercial du processeur (ex. « Intel(R) Core(TM) i9-14900K »), lu dans le registre Windows.
pub fn cpu_name() -> Option<String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
        let key: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0".encode_utf16().collect();
        let val: Vec<u16> = "ProcessorNameString\0".encode_utf16().collect();
        let mut buf = [0u16; 256];
        let mut bytes = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut bytes,
        );
        if r != 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let name = String::from_utf16_lossy(&buf[..len]).split_whitespace().collect::<Vec<_>>().join(" ");
        return (!name.is_empty()).then_some(name);
    }
    #[allow(unreachable_code)]
    None
}
