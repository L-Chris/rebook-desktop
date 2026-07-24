use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rebook_epub::EpubPublication;
use rebook_publication::BookSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LIBRARY_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "library.json";
const BOOKS_DIRECTORY: &str = "books";
const COVERS_DIRECTORY: &str = "covers";

pub type LibraryResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
pub struct LibraryBook {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub file_name: String,
    pub path: PathBuf,
    pub cover_bytes: Option<Vec<u8>>,
    pub added_at: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
    pub duplicates: usize,
}

pub struct LocalLibrary {
    root: PathBuf,
    books: Vec<LibraryBook>,
}

#[derive(Serialize, Deserialize)]
struct StoredLibrary {
    version: u32,
    #[serde(default)]
    books: Vec<StoredBook>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredBook {
    id: String,
    title: String,
    #[serde(default)]
    authors: Vec<String>,
    file_name: String,
    storage_name: String,
    cover_name: Option<String>,
    added_at: u64,
}

impl LocalLibrary {
    pub fn load_default() -> LibraryResult<Self> {
        let project = ProjectDirs::from("com", "Rebook", "Rebook")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法确定本地书架数据目录"))?;
        Self::load_from(project.data_local_dir().join("library"))
    }

    fn load_from(root: PathBuf) -> LibraryResult<Self> {
        fs::create_dir_all(root.join(BOOKS_DIRECTORY))?;
        fs::create_dir_all(root.join(COVERS_DIRECTORY))?;
        let manifest_path = root.join(MANIFEST_FILE);
        let stored = if manifest_path.exists() {
            let stored: StoredLibrary = serde_json::from_slice(&fs::read(&manifest_path)?)?;
            if stored.version != LIBRARY_VERSION {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("不支持的本地书架版本：{}", stored.version),
                )
                .into());
            }
            stored
        } else {
            StoredLibrary {
                version: LIBRARY_VERSION,
                books: Vec::new(),
            }
        };

        let mut books = stored
            .books
            .into_iter()
            .map(|book| {
                let cover_bytes = book
                    .cover_name
                    .as_ref()
                    .and_then(|name| fs::read(root.join(COVERS_DIRECTORY).join(name)).ok());
                LibraryBook {
                    id: book.id,
                    title: book.title,
                    authors: book.authors,
                    file_name: book.file_name,
                    path: root.join(BOOKS_DIRECTORY).join(book.storage_name),
                    cover_bytes,
                    added_at: book.added_at,
                }
            })
            .collect::<Vec<_>>();
        books.sort_by_key(|book| std::cmp::Reverse(book.added_at));
        Ok(Self { root, books })
    }

    pub fn books(&self) -> &[LibraryBook] {
        &self.books
    }

    pub fn import_files(&mut self, paths: &[PathBuf]) -> LibraryResult<ImportSummary> {
        let mut summary = ImportSummary::default();
        for path in paths {
            if self.import_file(path)? {
                summary.imported += 1;
            } else {
                summary.duplicates += 1;
            }
        }
        Ok(summary)
    }

    fn import_file(&mut self, source_path: &Path) -> LibraryResult<bool> {
        let bytes = fs::read(source_path)?;
        let id = format!("{:x}", Sha256::digest(&bytes));
        if self.books.iter().any(|book| book.id == id) {
            return Ok(false);
        }

        let publication = EpubPublication::open_bytes(Arc::<[u8]>::from(bytes.clone()))?;
        let metadata = &publication.book().metadata;
        let title = if metadata.title.trim().is_empty() {
            title_from_file_name(source_path)
        } else {
            metadata.title.trim().to_owned()
        };
        let file_name = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("book.epub")
            .to_owned();
        let storage_name = format!("{id}.epub");
        let cover_bytes = publication_cover_bytes(&publication);
        let cover_name = cover_bytes.as_ref().map(|_| format!("{id}.cover"));

        fs::write(self.root.join(BOOKS_DIRECTORY).join(&storage_name), bytes)?;
        if let (Some(name), Some(cover)) = (&cover_name, &cover_bytes) {
            fs::write(self.root.join(COVERS_DIRECTORY).join(name), cover)?;
        }

        self.books.insert(
            0,
            LibraryBook {
                id,
                title,
                authors: metadata.authors.clone(),
                file_name,
                path: self.root.join(BOOKS_DIRECTORY).join(storage_name),
                cover_bytes,
                added_at: unix_timestamp_millis(),
            },
        );
        self.persist()?;
        Ok(true)
    }

    pub fn remove(&mut self, id: &str) -> LibraryResult<bool> {
        let Some(index) = self.books.iter().position(|book| book.id == id) else {
            return Ok(false);
        };
        let book = self.books.remove(index);
        if let Err(error) = self.persist() {
            self.books.insert(index, book);
            return Err(error);
        }
        remove_if_exists(&book.path)?;
        remove_if_exists(
            &self
                .root
                .join(COVERS_DIRECTORY)
                .join(format!("{}.cover", book.id)),
        )?;
        Ok(true)
    }

    fn persist(&self) -> LibraryResult<()> {
        let stored = StoredLibrary {
            version: LIBRARY_VERSION,
            books: self
                .books
                .iter()
                .map(|book| StoredBook {
                    id: book.id.clone(),
                    title: book.title.clone(),
                    authors: book.authors.clone(),
                    file_name: book.file_name.clone(),
                    storage_name: book
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_owned(),
                    cover_name: book
                        .cover_bytes
                        .as_ref()
                        .map(|_| format!("{}.cover", book.id)),
                    added_at: book.added_at,
                })
                .collect(),
        };
        fs::write(
            self.root.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&stored)?,
        )?;
        Ok(())
    }
}

pub fn publication_cover_bytes(publication: &EpubPublication) -> Option<Vec<u8>> {
    publication
        .book()
        .cover
        .as_ref()
        .and_then(|href| publication.resource(href).ok())
        .map(|resource| resource.bytes.to_vec())
}

fn title_from_file_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("未命名书籍")
        .to_owned()
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const FIXTURE_ENTRIES: [&str; 5] = [
        "META-INF/container.xml",
        "OPS/package.opf",
        "OPS/nav.xhtml",
        "OPS/Styles/book.css",
        "OPS/Text/chapter.xhtml",
    ];

    #[test]
    fn manifest_round_trip_preserves_book_order_and_metadata() {
        let root = test_directory("manifest-round-trip");
        let mut library = LocalLibrary::load_from(root.clone()).unwrap();
        let managed_path = root.join(BOOKS_DIRECTORY).join("first.epub");
        fs::write(&managed_path, b"fixture").unwrap();
        library.books.push(LibraryBook {
            id: "first".into(),
            title: "第一本书".into(),
            authors: vec!["作者".into()],
            file_name: "source.epub".into(),
            path: managed_path,
            cover_bytes: None,
            added_at: 42,
        });
        library.persist().unwrap();

        let loaded = LocalLibrary::load_from(root.clone()).unwrap();
        assert_eq!(loaded.books.len(), 1);
        assert_eq!(loaded.books[0].title, "第一本书");
        assert_eq!(loaded.books[0].authors, ["作者"]);
        assert_eq!(loaded.books[0].file_name, "source.epub");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remove_deletes_only_the_managed_book() {
        let root = test_directory("remove-managed-book");
        let mut library = LocalLibrary::load_from(root.clone()).unwrap();
        let managed_path = root.join(BOOKS_DIRECTORY).join("managed.epub");
        fs::write(&managed_path, b"fixture").unwrap();
        library.books.push(LibraryBook {
            id: "managed".into(),
            title: "Managed".into(),
            authors: Vec::new(),
            file_name: "original.epub".into(),
            path: managed_path.clone(),
            cover_bytes: None,
            added_at: 1,
        });
        library.persist().unwrap();

        assert!(library.remove("managed").unwrap());
        assert!(!managed_path.exists());
        assert!(library.books.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_extracts_metadata_copies_content_and_skips_duplicates() {
        let root = test_directory("import-epub");
        let source = root.join("source.epub");
        build_fixture(&source);
        let mut library = LocalLibrary::load_from(root.join("data")).unwrap();

        let first = library.import_files(std::slice::from_ref(&source)).unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.duplicates, 0);
        assert_eq!(library.books[0].title, "Rebook 原生渲染样板");
        assert_eq!(library.books[0].authors, ["Rebook"]);
        assert!(library.books[0].path.exists());
        assert_ne!(library.books[0].path, source);

        let second = library.import_files(std::slice::from_ref(&source)).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.duplicates, 1);
        assert_eq!(library.books.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    fn build_fixture(output: &Path) {
        let fixture_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/minimal-epub");
        let mut archive = ZipWriter::new(fs::File::create(output).unwrap());
        archive
            .start_file(
                "mimetype",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        archive.write_all(b"application/epub+zip").unwrap();
        for entry in FIXTURE_ENTRIES {
            archive
                .start_file(
                    entry,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .unwrap();
            archive
                .write_all(&fs::read(fixture_root.join(entry)).unwrap())
                .unwrap();
        }
        archive.finish().unwrap();
    }

    fn test_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rebook-desktop-{name}-{}-{}",
            std::process::id(),
            unix_timestamp_millis()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
