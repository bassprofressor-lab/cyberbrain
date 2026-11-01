//! Git worktrees: one memory, one code tree each (0.9).
//!
//! A linked worktree (`git worktree add`) is a second checkout of the same repository,
//! often outside the project directory. Before this module two things went wrong there,
//! both without a word:
//!
//! - **No memory.** The store sits in the main checkout (`<project>/.cyberbrain`, gitignored),
//!   so walking up from a worktree elsewhere found none: `recall` refused and the hooks stood
//!   down for every session started in the worktree.
//! - **The wrong code.** With the store named by `--store`, `find` scanned the main checkout,
//!   which is another branch. The line ranges it handed back belonged to files the agent was
//!   not editing.
//!
//! The answer is the split ProjectAtlas makes (styler-ai, MIT: "each worktree keeps its own
//! map"), cut to what Cyberbrain has: notes are the project's and stay in the one store;
//! the code tree is the checkout's, so `find` reads the worktree the caller stands in.
//! `find` scans live and keeps no index, so nothing has to be built or kept per worktree.
//!
//! Git is read from the files it keeps, not by running `git`: a hook has a budget of
//! milliseconds and may run where no `git` is on the path. Only the plain layouts are
//! understood (a `.git` directory, or a `.git` file naming the worktree's admin directory
//! with a `commondir` inside); anything else is "not a git checkout", which leaves the old
//! behaviour in place.

use std::path::{Path, PathBuf};

/// The git checkout a directory belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// The top of this checkout: where its `.git` entry is.
    pub top: PathBuf,
    /// The repository's shared git directory (`<main>/.git`). Equal for every worktree of
    /// one repository, which is what makes two checkouts "the same project".
    pub common: PathBuf,
    /// Whether this is a linked worktree rather than the main checkout.
    pub linked: bool,
}

impl Checkout {
    /// The main checkout: the directory holding the shared git directory. `None` for a bare
    /// repository, whose git directory is not called `.git` and has no checkout around it.
    pub fn main_top(&self) -> Option<PathBuf> {
        (self.common.file_name()? == ".git").then(|| self.common.parent().map(Path::to_path_buf))?
    }
}

/// The checkout `dir` is in, or `None` when it is in none this module understands.
pub fn locate(dir: &Path) -> Option<Checkout> {
    for top in dir.ancestors() {
        let dot_git = top.join(".git");
        let Ok(md) = std::fs::symlink_metadata(&dot_git) else {
            continue;
        };
        if md.is_dir() {
            return Some(Checkout {
                top: top.to_path_buf(),
                common: canonical(&dot_git),
                linked: false,
            });
        }
        if md.is_file() {
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let admin = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let admin = top.join(admin);
            // A submodule's `.git` file points into the parent's modules/ and has no
            // commondir: it is its own repository, not a worktree of anything here.
            let common = std::fs::read_to_string(admin.join("commondir")).ok()?;
            let common = admin.join(common.trim());
            return Some(Checkout {
                top: top.to_path_buf(),
                common: canonical(&common),
                linked: true,
            });
        }
    }
    None
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Where to look for the store when walking up from `dir` found none: the main checkout,
/// if `dir` is in a linked worktree. The caller walks up from there as it did from `dir`.
pub fn main_checkout_for(dir: &Path) -> Option<PathBuf> {
    let c = locate(dir)?;
    if !c.linked {
        return None;
    }
    c.main_top()
}

/// The code tree for `find`, when the caller stands in a different checkout of the same
/// repository as `project`: that checkout's top. `None` keeps `project`.
pub fn code_root_for(project: &Path, cwd: &Path) -> Option<PathBuf> {
    let here = locate(cwd)?;
    let there = locate(project)?;
    (here.common == there.common && canonical(&here.top) != canonical(&there.top))
        .then_some(here.top)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The layout `git worktree add` produces, written by hand so the tests need no git:
    /// `<main>/.git/worktrees/<name>/{gitdir,commondir}` and `<wt>/.git` naming it.
    fn repo_with_worktree(base: &Path) -> (PathBuf, PathBuf) {
        let main = base.join("main");
        let wt = base.join("elsewhere").join("wt");
        let admin = main.join(".git").join("worktrees").join("wt");
        fs::create_dir_all(&admin).unwrap();
        fs::create_dir_all(main.join("src")).unwrap();
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        (main, wt)
    }

    #[test]
    fn a_linked_worktree_is_found_with_the_main_checkouts_git_directory() {
        let d = tempfile::tempdir().unwrap();
        let (main, wt) = repo_with_worktree(d.path());
        let c = locate(&wt.join("src")).unwrap();
        assert!(c.linked);
        assert_eq!(c.top, wt);
        assert_eq!(c.common, canonical(&main.join(".git")));
        assert_eq!(c.main_top().map(|p| canonical(&p)), Some(canonical(&main)));
        let m = locate(&main.join("src")).unwrap();
        assert!(!m.linked);
        assert_eq!(m.common, c.common);
    }

    #[test]
    fn the_main_checkout_is_offered_only_from_a_linked_worktree() {
        let d = tempfile::tempdir().unwrap();
        let (main, wt) = repo_with_worktree(d.path());
        assert_eq!(
            main_checkout_for(&wt.join("src")).map(|p| canonical(&p)),
            Some(canonical(&main))
        );
        assert_eq!(main_checkout_for(&main.join("src")), None);
        assert_eq!(main_checkout_for(d.path()), None);
    }

    #[test]
    fn find_reads_the_worktree_it_stands_in_and_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let (main, wt) = repo_with_worktree(d.path());
        assert_eq!(code_root_for(&main, &wt.join("src")), Some(wt.clone()));
        // In the main checkout, or outside any checkout, the project stays the root.
        assert_eq!(code_root_for(&main, &main.join("src")), None);
        assert_eq!(code_root_for(&main, d.path()), None);
        // Another repository is not a worktree of this one.
        let other = d.path().join("other");
        fs::create_dir_all(other.join(".git")).unwrap();
        assert_eq!(code_root_for(&main, &other), None);
    }

    #[test]
    fn a_submodule_or_a_broken_git_file_is_not_a_worktree() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("sub");
        let modules = d.path().join("parent-git").join("modules").join("sub");
        fs::create_dir_all(&modules).unwrap();
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join(".git"), format!("gitdir: {}\n", modules.display())).unwrap();
        assert_eq!(locate(&sub), None);
        fs::write(sub.join(".git"), "nonsense\n").unwrap();
        assert_eq!(locate(&sub), None);
    }
}
