use std::io;
use std::sync::{Arc, Mutex};
use utils::eventfd::EventFd;
use crate::devices::virtio::net::TapError;
use crate::devices::virtio::queue::Queue;

mod event_handler;
mod device;
mod metrics;
mod persist;


#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum VhostNetError {
    /// Open tap device failed: {0}
    TapOpen(TapError),
    /// Setting tap interface offload flags failed: {0}
    TapSetOffload(TapError),
    /// Setting vnet header size failed: {0}
    TapSetVnetHdrSize(TapError),
    /// EventFd error: {0}
    EventFd(io::Error),
    /// IO error: {0}
    IO(io::Error),
    /// The VNET header is missing from the frame
    VnetHeaderMissing,
}

pub trait VhostKernHandleBackend: Sized {
    fn set_owner(&self) -> Result<(), VhostNetError>;

    fn reset_owner(&self) -> Result<(), VhostNetError>;
    fn get_features(&self) -> Result<u64, VhostNetError>;

    fn set_features(&self, features: u64) -> Result<(), VhostNetError>;
    fn set_mem_table(&self) -> Result<(), VhostNetError>;

    fn set_vring_num(&self, queue_idx: usize, num: u16) -> Result<(), VhostNetError>;

    fn set_vring_base(&self, queue_idx: usize, last_avail_idx: u16) -> Result<(), VhostNetError>;
    fn get_vring_base(&self, queue_idx: usize) -> Result<u16, VhostNetError>;

    fn set_vring_call(&self, queue_idx: usize, fd: Arc<EventFd>) -> Result<(), VhostNetError>;

    fn set_vring_kick(&self, queue_idx: usize, fd: Arc<EventFd>) -> Result<(), VhostNetError>;
    fn set_vring_enable(&self, _queue_idx: usize, _status: bool) -> Result<(), VhostNetError> {
        Ok(())
    }
}