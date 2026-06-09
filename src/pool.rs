use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct ThreadPool {
    workers: Vec<JoinHandle<()>>,
    tx: Option<Sender<Job>>,
}

impl ThreadPool {
    pub fn new(threads: usize) -> Self {
        assert!(threads > 0);
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let workers = (0..threads)
            .map(|_| {
                let rx = Arc::clone(&rx);
                std::thread::spawn(move || worker_loop(rx))
            })
            .collect();
        Self {
            workers,
            tx: Some(tx),
        }
    }

    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.tx
            .as_ref()
            .expect("pool is shutting down")
            .send(Box::new(f))
            .expect("workers have exited");
    }
}

fn worker_loop(rx: Arc<Mutex<Receiver<Job>>>) {
    loop {
        let job = rx.lock().unwrap().recv();
        match job {
            Ok(job) => job(),
            Err(_) => break,
        }
    }
}
