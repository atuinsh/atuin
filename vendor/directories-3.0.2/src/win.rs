extern crate dirs_sys;

use std::path::PathBuf;
use std::iter::FromIterator;

use BaseDirs;
use UserDirs;
use ProjectDirs;

pub fn base_dirs() -> Option<BaseDirs> {
    let home_dir       = dirs_sys::known_folder_profile();
    let data_dir       = dirs_sys::known_folder_roaming_app_data();
    let data_local_dir = dirs_sys::known_folder_local_app_data();
    if let (Some(home_dir), Some(data_dir), Some(data_local_dir)) = (home_dir, data_dir, data_local_dir) {
        let cache_dir      = data_local_dir.clone();
        let config_dir     = data_dir.clone();
        let preference_dir = data_dir.clone();

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
    if let Some(home_dir) = dirs_sys::known_folder_profile() {
        let audio_dir     = dirs_sys::known_folder_music();
        let desktop_dir   = dirs_sys::known_folder_desktop();
        let document_dir  = dirs_sys::known_folder_documents();
        let download_dir  = dirs_sys::known_folder_downloads();
        let picture_dir   = dirs_sys::known_folder_pictures();
        let public_dir    = dirs_sys::known_folder_public();
        let template_dir  = dirs_sys::known_folder_templates();
        let video_dir     = dirs_sys::known_folder_videos();

        let user_dirs = UserDirs {
            home_dir:     home_dir,
            audio_dir:    audio_dir,
            desktop_dir:  desktop_dir,
            document_dir: document_dir,
            download_dir: download_dir,
            font_dir:     None,
            picture_dir:  picture_dir,
            public_dir:   public_dir,
            template_dir: template_dir,
            video_dir:    video_dir
        };
        Some(user_dirs)
    } else {
        None
    }
}

pub fn project_dirs_from_path(project_path: PathBuf) -> Option<ProjectDirs> {
    let app_data_local   = dirs_sys::known_folder_local_app_data();
    let app_data_roaming = dirs_sys::known_folder_roaming_app_data();
    if let (Some(app_data_local), Some(app_data_roaming)) = (app_data_local, app_data_roaming) {
        let app_data_local   = app_data_local.join(&project_path);
        let app_data_roaming = app_data_roaming.join(&project_path);
        let cache_dir        = app_data_local.join("cache");
        let data_local_dir   = app_data_local.join("data");
        let config_dir       = app_data_roaming.join("config");
        let data_dir         = app_data_roaming.join("data");
        let preference_dir   = config_dir.clone();

        let project_dirs = ProjectDirs {
            project_path:   project_path,
            cache_dir:      cache_dir,
            config_dir:     config_dir,
            data_dir:       data_dir,
            data_local_dir: data_local_dir,
            preference_dir: preference_dir,
            runtime_dir:    None
        };
        Some(project_dirs)
    } else {
        None
    }

}

pub fn project_dirs_from(_qualifier: &str, organization: &str, application: &str) -> Option<ProjectDirs> {
    ProjectDirs::from_path(PathBuf::from_iter(&[organization, application]))
}
