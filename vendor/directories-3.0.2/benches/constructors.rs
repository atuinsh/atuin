#[macro_use]
extern crate bencher;
extern crate directories;

use bencher::Bencher;
use bencher::black_box;
use directories::BaseDirs;
use directories::ProjectDirs;
use directories::UserDirs;

fn base_dirs(b: &mut Bencher) {
    b.iter(|| {
        let _ = black_box(BaseDirs::new());
    });
}

fn user_dirs(b: &mut Bencher) {
    b.iter(|| {
        let _ = black_box(UserDirs::new());
    });
}

fn project_dirs_from_path(b: &mut Bencher) {
    b.iter(|| {
        let _ = black_box(ProjectDirs::from_path(Default::default()));
    });
}

fn project_dirs(b: &mut Bencher) {
    b.iter(|| {
        let _ = black_box(ProjectDirs::from("org", "foo", "Bar App"));
    });
}

benchmark_group!(constructors,
    base_dirs,
    user_dirs,
    project_dirs_from_path,
    project_dirs,
);
benchmark_main!(constructors);
