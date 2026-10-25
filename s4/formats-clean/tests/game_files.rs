mod common;
use common::*;
use s4_formats_clean::archive::Archive;
use s4_formats_clean::game_files::GameFiles;
use std::path::{Path, PathBuf};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("s4cr-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn put(&self, rel: &str, data: &[u8]) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn vec_file(name: &str) -> Vec<u8> {
    std::fs::read(vectors_dir().join("data").join(name)).unwrap()
}

/// (archive bytes, name of its first entry, that entry's contents)
fn sample(name: &str) -> (Vec<u8>, String, Vec<u8>) {
    let bytes = vec_file(name);
    let a = Archive::parse(&bytes).unwrap();
    let e = a.entries.iter().find(|e| e.size > 0).unwrap();
    let data = a.read(e).unwrap();
    (bytes.clone(), e.path.clone(), data)
}

#[test]
fn open_never_fails_for_a_missing_folder() {
    let g = GameFiles::open(Path::new("/nonexistent/s4cr/folder")).unwrap();
    assert_eq!(g.read("Gfx\\10.gil"), None);
}

#[test]
fn reads_from_one_archive_with_any_slash_and_case() {
    let (lib, path, data) = sample("archive-compressed-and-stored.lib");
    let d = Dir::new("one");
    d.put("game.lib", &lib);
    let g = GameFiles::open(&d.0).unwrap();
    assert_eq!(g.read(&path), Some(data.clone()));
    assert_eq!(g.read(&path.replace('\\', "/")), Some(data.clone()));
    assert_eq!(g.read(&path.to_uppercase()), Some(data.clone()));
    assert_eq!(g.read(&path.to_lowercase()), Some(data));
    assert_eq!(g.read("no\\such\\file.bin"), None);
    assert_eq!(g.read(""), None);
}

#[test]
fn loose_files_come_first_and_both_slashes_work() {
    let (lib, path, data) = sample("archive-compressed-and-stored.lib");
    let d = Dir::new("loose");
    d.put("game.lib", &lib);
    let rel = path.replace('\\', "/");
    d.put(&rel, b"loose copy");
    d.put("Folder/a.bin", &[1, 2, 3]);
    let g = GameFiles::open(&d.0).unwrap();
    assert_eq!(g.read(&path), Some(b"loose copy".to_vec()));
    assert_eq!(g.read(&rel), Some(b"loose copy".to_vec()));
    assert_eq!(g.read("Folder\\a.bin"), Some(vec![1, 2, 3]));
    assert_eq!(g.read("Folder/a.bin"), Some(vec![1, 2, 3]));
    // the match of a loose file is the file system's (the folder here is spelled as written)
    let _ = data;
}

#[test]
fn a_directory_is_not_a_file() {
    let (lib, path, data) = sample("archive-compressed-and-stored.lib");
    let d = Dir::new("dir");
    d.put("game.lib", &lib);
    std::fs::create_dir_all(d.0.join(path.replace('\\', "/"))).unwrap();
    let g = GameFiles::open(&d.0).unwrap();
    assert_eq!(g.read(&path), Some(data));
}

#[test]
fn both_archives_game_lib_first() {
    let (a, a_path, a_data) = sample("archive-compressed-and-stored.lib");
    let (b, b_path, b_data) = sample("archive-many-files.lib");
    let d = Dir::new("two");
    d.put("game.lib", &a);
    d.put("gfx.lib", &b);
    let g = GameFiles::open(&d.0).unwrap();
    assert_eq!(g.read(&a_path), Some(a_data.clone()));
    assert_eq!(g.read(&b_path), Some(b_data.clone()));
    // the same archive in both places: the first one wins (swap the contents to see which)
    let d2 = Dir::new("same-name");
    d2.put("game.lib", &a);
    d2.put("gfx.lib", &a);
    assert_eq!(GameFiles::open(&d2.0).unwrap().read(&a_path), Some(a_data));
}

#[test]
fn archives_that_do_not_parse_are_ignored() {
    let (b, b_path, b_data) = sample("archive-many-files.lib");
    let d = Dir::new("bad-first");
    d.put("game.lib", &[1, 2, 3, 4, 5]);
    d.put("gfx.lib", &b);
    assert_eq!(GameFiles::open(&d.0).unwrap().read(&b_path), Some(b_data.clone()));
    let d = Dir::new("bad-second");
    d.put("game.lib", &b);
    d.put("gfx.lib", &[0u8; 100]);
    assert_eq!(GameFiles::open(&d.0).unwrap().read(&b_path), Some(b_data));
    let d = Dir::new("empty");
    d.put("game.lib", &[]);
    assert_eq!(GameFiles::open(&d.0).unwrap().read(&b_path), None);
}

#[test]
fn only_the_two_archive_names_are_mounted() {
    let (b, b_path, _) = sample("archive-many-files.lib");
    let d = Dir::new("other-name");
    d.put("other.lib", &b);
    d.put("GAME.LIB.bak", &b);
    assert_eq!(GameFiles::open(&d.0).unwrap().read(&b_path), None);
}
