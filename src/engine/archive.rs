//! Unpacking of downloaded archives, in process so that no `tar` or `unzip` has
//! to be on the machine, or to behave the same on all of them.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use tar::Archive;
use zip::ZipArchive;

/// How an unpacking went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    /// Entries left out, as nothing hodis installs reads them.
    pub skipped: Vec<String>,
}

/// Unpacks `bytes` into `into`, as a zip if `name` says so and otherwise as a
/// gzipped tar.
pub fn unpack(name: &str, bytes: &[u8], into: &Path) -> Result<Unpacked> {
    fs::create_dir_all(into).with_context(|| format!("cannot create {}", into.display()))?;
    let zip = Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    if zip {
        unpack_zip(bytes, into).with_context(|| format!("cannot unpack {name}"))
    } else {
        unpack_tar_gz(bytes, into).with_context(|| format!("cannot unpack {name}"))
    }
}

/// Leaves out symlinks, which a standard Windows account may not create, and
/// names like `a:b`, a data stream there. SearXNG's archive has both, unused.
fn unpack_tar_gz(bytes: &[u8], into: &Path) -> Result<Unpacked> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let mut unpacked = Unpacked {
        files: 0,
        skipped: Vec::new(),
    };

    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        // Global headers carry metadata, such as the commit, rather than files.
        if kind.is_pax_global_extensions() {
            continue;
        }
        let path = entry.path()?.into_owned();
        if !(kind.is_file() || kind.is_dir()) || !fits_any_system(&path) {
            unpacked.skipped.push(path.display().to_string());
            continue;
        }
        // False for a path leading out of `into`, which is left out too.
        if entry
            .unpack_in(into)
            .with_context(|| format!("cannot write {}", path.display()))?
        {
            unpacked.files += usize::from(kind.is_file());
        } else {
            unpacked.skipped.push(path.display().to_string());
        }
    }
    Ok(unpacked)
}

fn unpack_zip(bytes: &[u8], into: &Path) -> Result<Unpacked> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let files = archive.len();
    archive.extract(into)?;
    Ok(Unpacked {
        files,
        skipped: Vec::new(),
    })
}

/// Whether every part of `path` is a name Windows can hold too.
fn fits_any_system(path: &Path) -> bool {
    path.iter().all(|part| {
        part.to_str().is_some_and(|part| {
            !part
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Cursor, Write};

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, EntryType, Header};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::unpack;

    fn entry(kind: EntryType, size: u64, mode: u32) -> Header {
        let mut header = Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(size);
        header.set_mode(mode);
        header
    }

    fn file(tar: &mut Builder<impl Write>, path: &str, text: &str) {
        let mut header = entry(EntryType::Regular, text.len() as u64, 0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, text.as_bytes())
            .expect("the entry is appended");
    }

    /// A raw name, which `append_data` would refuse.
    fn with_raw_name(tar: &mut Builder<impl Write>, path: &str, text: &str) {
        let mut header = entry(EntryType::Regular, text.len() as u64, 0o644);
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        tar.append(&header, text.as_bytes())
            .expect("the entry is appended");
    }

    fn gzipped(build: impl FnOnce(&mut Builder<GzEncoder<Vec<u8>>>)) -> Vec<u8> {
        let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        build(&mut tar);
        tar.into_inner()
            .expect("the archive closes")
            .finish()
            .expect("the archive compresses")
    }

    #[test]
    fn a_tar_unpacks_its_files_and_directories() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let bytes = gzipped(|tar| {
            let mut dir = entry(EntryType::Directory, 0, 0o755);
            dir.set_cksum();
            tar.append_data(&mut dir, "tree/", &[][..])
                .expect("the entry is appended");
            file(tar, "tree/setup.py", "print()");
        });

        let unpacked = unpack("a.tar.gz", &bytes, scratch.path()).expect("it unpacks");

        assert_eq!(unpacked.files, 1);
        assert_eq!(unpacked.skipped, Vec::<String>::new());
        assert_eq!(
            fs::read_to_string(scratch.path().join("tree/setup.py")).expect("the file exists"),
            "print()"
        );
    }

    #[test]
    fn a_symlink_is_left_out_rather_than_failing_the_install() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let bytes = gzipped(|tar| {
            let mut link = entry(EntryType::Symlink, 0, 0o777);
            link.set_cksum();
            tar.append_link(&mut link, "tree/apache2", "httpd")
                .expect("the entry is appended");
            file(tar, "tree/kept", "kept");
        });

        let unpacked = unpack("a.tar.gz", &bytes, scratch.path()).expect("it unpacks");

        assert_eq!(unpacked.skipped, ["tree/apache2"]);
        assert!(!scratch.path().join("tree/apache2").exists());
        assert!(scratch.path().join("tree/kept").is_file());
    }

    #[test]
    fn a_name_windows_cannot_hold_is_left_out_on_every_system() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let bytes = gzipped(|tar| {
            with_raw_name(tar, "tree/searxng.conf:socket", "x");
            file(tar, "tree/searxng.conf", "kept");
        });

        let unpacked = unpack("a.tar.gz", &bytes, scratch.path()).expect("it unpacks");

        assert_eq!(unpacked.skipped, ["tree/searxng.conf:socket"]);
        assert_eq!(unpacked.files, 1);
    }

    #[test]
    fn a_path_out_of_the_target_is_left_out() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let target = scratch.path().join("target");
        let bytes = gzipped(|tar| with_raw_name(tar, "../escaped", "x"));

        // The tar crate refuses to unpack such a path at all or skips it; what
        // matters is that nothing lands outside the target.
        let _ = unpack("a.tar.gz", &bytes, &target);

        assert!(!scratch.path().join("escaped").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_stays_executable() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = tempfile::tempdir().expect("a scratch directory");
        let bytes = gzipped(|tar| {
            let mut header = entry(EntryType::Regular, 2, 0o755);
            header.set_cksum();
            tar.append_data(&mut header, "uv", &b"#!"[..])
                .expect("the entry is appended");
        });

        unpack("a.tar.gz", &bytes, scratch.path()).expect("it unpacks");

        let mode = fs::metadata(scratch.path().join("uv"))
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "mode {mode:o}");
    }

    #[test]
    fn a_zip_unpacks_too() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("uv.exe", SimpleFileOptions::default())
            .expect("the entry starts");
        zip.write_all(b"MZ").expect("the entry is written");
        let bytes = zip.finish().expect("the archive closes").into_inner();

        let unpacked = unpack("uv.zip", &bytes, scratch.path()).expect("it unpacks");

        assert_eq!(unpacked.files, 1);
        assert!(scratch.path().join("uv.exe").is_file());
    }

    #[test]
    fn a_damaged_archive_is_an_error_naming_it() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let error = unpack("broken.tar.gz", b"not an archive", scratch.path())
            .expect_err("it cannot unpack");
        assert!(format!("{error:#}").contains("broken.tar.gz"), "{error:#}");
    }
}
