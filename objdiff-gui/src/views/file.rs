use std::{future::Future, path::PathBuf, pin::Pin, thread::JoinHandle};

use objdiff_core::config::path::check_path_buf;
use pollster::FutureExt;
use rfd::FileHandle;
use typed_path::Utf8PlatformPathBuf;

#[derive(Default)]
pub enum FileDialogResult {
    #[default]
    None,
    ProjectDir(Utf8PlatformPathBuf),
    TargetDir(Utf8PlatformPathBuf),
    BaseDir(Utf8PlatformPathBuf),
    Object(Utf8PlatformPathBuf),
}

#[derive(Default)]
pub struct FileDialogState {
    thread: Option<JoinHandle<FileDialogResult>>,
}

impl FileDialogState {
    pub fn queue<InitCb, ResultCb>(&mut self, init: InitCb, result_cb: ResultCb)
    where
        InitCb: FnOnce() -> Pin<Box<dyn Future<Output = Option<FileHandle>> + Send>>,
        ResultCb: FnOnce(Utf8PlatformPathBuf) -> FileDialogResult + Send + 'static,
    {
        if self.thread.is_some() {
            return;
        }
        let future = init();
        self.thread = Some(std::thread::spawn(move || {
            if let Some(handle) = future.block_on() {
                let path = PathBuf::from(handle);
                check_path_buf(path).map(result_cb).unwrap_or(FileDialogResult::None)
            } else {
                FileDialogResult::None
            }
        }));
    }

    pub fn poll(&mut self) -> FileDialogResult {
        if let Some(thread) = &mut self.thread {
            if thread.is_finished() {
                self.thread.take().unwrap().join().unwrap_or(FileDialogResult::None)
            } else {
                FileDialogResult::None
            }
        } else {
            FileDialogResult::None
        }
    }
}
