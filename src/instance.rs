use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(directory: &Path) -> io::Result<Self> {
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
        let metadata = fs::symlink_metadata(directory)?;
        // В каталоге блокировки нельзя позволять заменять inode файла,
        // иначе два процесса смогут удерживать разные блокировки одного имени.
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "небезопасный каталог блокировки",
            ));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("daemon.lock"))?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "небезопасный файл блокировки",
            ));
        }
        file.try_lock().map_err(|err| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("другой экземпляр punto-rs уже запущен или блокировка недоступна: {err}"),
            )
        })?;
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_rejects_second_owner_and_can_be_reused_after_drop() {
        let dir = std::env::temp_dir().join(format!("punto-lock-test-{}", std::process::id()));
        let first = InstanceLock::acquire(&dir).unwrap();
        assert!(InstanceLock::acquire(&dir).is_err());
        drop(first);
        drop(InstanceLock::acquire(&dir).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
