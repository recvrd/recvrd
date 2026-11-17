use flume::Sender;
use log::{error, info};
use rand::seq::SliceRandom;
use ytpapi2::YoutubeMusicVideoRef;

use crate::{
    DATABASE,
    consts::{CACHE_DIR, CONFIG},
    run_service,
    structures::performance,
    term::{ManagerMessage, Screens},
};

pub fn spawn_local_musics_task(updater_s: Sender<ManagerMessage>) {
    run_service(async move {
        info!("Database getter task on");
        let guard = performance::guard("Local musics");
        match DATABASE.read() {
            Ok(Some(videos)) => shuffle_and_send(videos, &updater_s),
            Ok(None) => rebuild_from_files(&updater_s),
            Err(error) => {
                error!("database read failed: {error}; rebuild from files");
                rebuild_from_files(&updater_s);
            }
        }
        drop(guard);
    });
}

fn rebuild_from_files(updater_s: &Sender<ManagerMessage>) {
    let mut videos = Vec::new();
    for files in std::fs::read_dir(CACHE_DIR.join("downloads")).unwrap() {
        let path = files.unwrap().path();
        if path.as_os_str().to_string_lossy().ends_with(".json") {
            let video =
                serde_json::from_str(std::fs::read_to_string(path).unwrap().as_str()).unwrap();
            videos.push(video);
        }
    }
    shuffle_and_send(videos, updater_s);

    if let Err(error) = DATABASE.write() {
        error!("could not save database: {error}");
    }
}

fn shuffle_and_send(mut videos: Vec<YoutubeMusicVideoRef>, updater_s: &Sender<ManagerMessage>) {
    DATABASE.clone_from(&videos);

    if CONFIG.player.shuffle {
        videos.shuffle(&mut rand::thread_rng());
    }

    updater_s
        .send(
            ManagerMessage::AddElementToChooser(("Local musics".to_owned(), videos))
                .pass_to(Screens::Playlist),
        )
        .unwrap();
}
