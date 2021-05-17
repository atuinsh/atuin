extern crate dirs_sys;

use std::path::PathBuf;

use BaseDirs;
use UserDirs;
use ProjectDirs;

pub fn base_dirs() -> Option<BaseDirs> {
    if let Some(home_dir)  = dirs_sys::home_dir() {
        let cache_dir      = home_dir.join("Library/Caches");
        let config_dir     = home_dir.join("Library/Application Support");
        let data_dir       = home_dir.join("Library/Application Support");
        let data_local_dir = data_dir.clone();
        let preference_dir = home_dir.join("Library/Preferences");

        let base_dirs = BaseDirs {
            home_dir:       home_dir,
            cache_dir:      cache_dir,
            config_dir:     config_dir,
            data_dir:       data_dir,
            data_local_dir: data_local_dir,
            executable_dir: None,
            preference_dir: preference_dir,
            runtime_dir:    None
        };
        Some(base_dirs)
    } else {
        None
    }
}

pub fn user_dirs() -> Option<UserDirs> {
    if let Some(home_dir) = dirs_sys::home_dir() {
        let audio_dir     = home_dir.join("Music");
        let desktop_dir   = home_dir.join("Desktop");
        let document_dir  = home_dir.join("Documents");
        let download_dir  = home_dir.join("Downloads");
        let picture_dir   = home_dir.join("Pictures");
        let public_dir    = home_dir.join("Public");
        let video_dir     = home_dir.join("Movies");
        let font_dir      = home_dir.join("Library/Fonts");

        let user_dirs = UserDirs {
            home_dir:     home_dir,
            audio_dir:    Some(audio_dir),
            desktop_dir:  Some(desktop_dir),
            document_dir: Some(document_dir),
            download_dir: Some(download_dir),
            font_dir:     Some(font_dir),
            picture_dir:  Some(picture_dir),
            public_dir:   Some(public_dir),
            template_dir: None,
            video_dir:    Some(video_dir)
        };
        Some(user_dirs)
    } else {
        None
    }
}

pub fn project_dirs_from_path(project_path: PathBuf) -> Option<ProjectDirs> {
    if let Some(home_dir)  = dirs_sys::home_dir() {
        let cache_dir      = home_dir.join("Library/Caches").join(&project_path);
        let config_dir     = home_dir.join("Library/Application Support").join(&project_path);
        let data_dir       = home_dir.join("Library/Application Support").join(&project_path);
        let data_local_dir = data_dir.clone();
        let preference_dir = home_dir.join("Library/Preferences").join(&project_path);

        let project_dirs = ProjectDirs {
            project_path:   project_path,
            cache_dir:      cache_dir,
            config_dir:     config_dir,
            data_dir:       data_dir,
            data_local_dir: data_local_dir,
            preference_dir: preference_dir,
            runtime_dir:    None,
        };
        Some(project_dirs)
    } else {
        None
    }
}

pub fn project_dirs_from(qualifier: &str, organization: &str, application: &str) -> Option<ProjectDirs> {
    // we should replace more characters, according to RFC1034 identifier rules
    let organization = organization.replace(" ", "-");
    let application  = application.replace(" ", "-");
    let mut parts    = vec![qualifier, &organization, &application]; parts.retain(|e| !e.is_empty());
    let bundle_id    = parts.join(".");
    ProjectDirs::from_path(PathBuf::from(bundle_id))
}
