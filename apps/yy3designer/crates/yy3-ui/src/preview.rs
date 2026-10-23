use std::sync::{Arc, mpsc};
use yy3_core::{
    Object,
    image::Raster,
    mesh::{self, Mesh},
};
pub struct Request {
    pub id: u64,
    pub object: Arc<Object>,
    pub width: u32,
    pub height: u32,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}
pub struct Response {
    pub id: u64,
    pub object: Arc<Object>,
    pub result: Result<(Arc<Mesh>, Raster), String>,
}
pub struct Worker {
    sender: Option<mpsc::SyncSender<Request>>,
    pub receiver: mpsc::Receiver<Response>,
    thread: Option<std::thread::JoinHandle<()>>,
}
struct Cached {
    object: Arc<Object>,
    mesh: Result<Arc<Mesh>, String>,
}
impl Worker {
    pub fn new() -> Self {
        let (sender, input) = mpsc::sync_channel::<Request>(1);
        let (output, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut cached: Option<Cached> = None;
            while let Ok(r) = input.recv() {
                if cached
                    .as_ref()
                    .is_none_or(|c| !Arc::ptr_eq(&c.object, &r.object))
                {
                    cached = Some(Cached {
                        object: r.object.clone(),
                        mesh: mesh::build(&r.object)
                            .map(Arc::new)
                            .map_err(|e| e.to_string()),
                    });
                }
                let result = cached.as_ref().unwrap().mesh.clone().and_then(|mesh| {
                    mesh::render(&mesh, r.width, r.height, r.yaw, r.pitch, r.zoom)
                        .map(|image| (mesh, image))
                        .map_err(|e| e.to_string())
                });
                if output
                    .send(Response {
                        id: r.id,
                        object: r.object,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            sender: Some(sender),
            receiver,
            thread: Some(thread),
        }
    }
    pub fn submit(&self, request: Request) -> Option<Request> {
        match self.sender.as_ref()?.try_send(request) {
            Ok(()) => None,
            Err(mpsc::TrySendError::Full(r)) => Some(r),
            Err(mpsc::TrySendError::Disconnected(_)) => None,
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
