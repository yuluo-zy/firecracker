use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};
use event_manager::SubscriberId;
use serde::{Deserialize, Serialize};
use vm_memory::{Address, GuestAddress};
use utils::{ioctl_io_nr, ioctl_ior_nr, ioctl_iow_nr, ioctl_iowr_nr};
use utils::eventfd::EventFd;
use utils::ioctl::ioctl_with_ref;
use utils::net::mac::MacAddr;
use crate::devices::virtio::block::vhost_user::device::VhostUserBlockImpl;
use crate::devices::virtio::device::{DeviceState, IrqTrigger};
use crate::devices::virtio::net::device::ConfigSpace;
use crate::devices::virtio::net::Tap;
use crate::devices::virtio::net::vhost::{VhostKernHandleBackend, VhostNetError};
use crate::devices::virtio::vhost_user::VhostUserHandleBackend;

pub(crate) const VHOST: u32 = 0xaf;
/// 驱动配置
// ioctl_ior_nr!(VHOST_GET_FEATURES, VHOST, 0x00, u64);
// ioctl_iow_nr!(VHOST_SET_FEATURES, VHOST, 0x00, u64);
// ioctl_io_nr!(VHOST_SET_OWNER, VHOST, 0x01);
// ioctl_io_nr!(VHOST_RESET_OWNER, VHOST, 0x02);
// ioctl_iow_nr!(VHOST_SET_MEM_TABLE, VHOST, 0x03, VhostMemory);
// ioctl_iow_nr!(VHOST_SET_VRING_NUM, VHOST, 0x10, VhostVringState);
// ioctl_iow_nr!(VHOST_SET_VRING_ADDR, VHOST, 0x11, VhostVringAddr);
// ioctl_iow_nr!(VHOST_SET_VRING_BASE, VHOST, 0x12, VhostVringState);
// ioctl_iowr_nr!(VHOST_GET_VRING_BASE, VHOST, 0x12, VhostVringState);
// ioctl_iow_nr!(VHOST_SET_VRING_KICK, VHOST, 0x20, VhostVringFile);
// ioctl_iow_nr!(VHOST_SET_VRING_CALL, VHOST, 0x21, VhostVringFile);
// ioctl_iow_nr!(VHOST_NET_SET_BACKEND, VHOST, 0x30, VhostVringFile);
// ioctl_iow_nr!(VHOST_VSOCK_SET_GUEST_CID, VHOST, 0x60, u64);
// ioctl_iow_nr!(VHOST_VSOCK_SET_RUNNING, VHOST, 0x61, i32);
//
// #[repr(C)]
// #[derive(Debug, Copy, Clone)]
// pub struct VhostVringFile{
//     pub index: u32,
//     pub fd: RawFd
// }
// #[repr(C)]
// #[derive(Debug, Copy, Clone)]
// pub struct VhostVringState{
//     index: u32,
//     num: u32
// }
// #[repr(C)]
// #[derive(Debug, Copy, Clone)]
// pub struct VhostVringAddr{
//     /// Vring index.
//     index: u32,
//     /// Option flags.
//     flags: u32,
//     /// Base address of descriptor table.
//     desc_user_addr: u64,
//     /// Base address of used vring.
//     used_user_addr: u64,
//     /// Base address of available vring.
//     avail_user_addr: u64,
//     /// Address where to write logs.
//     log_guest_addr: u64,
// }
//
//
// #[repr(C)]
// #[derive(Debug, Copy, Clone, Default)]
// pub struct VhostMemory{
//     // 内存区域的数量。
//     nregions: u32,
//     // 填充字段，用于对齐结构体
//     padding: u32,
// }
//
// #[repr(C)]
// #[derive(Debug, Copy, Clone, Default)]
// struct VhostMemoryRegion {
//     /// GPA. 来宾物理地址。它是虚拟机中的物理地址
//     guest_phys_addr: u64,
//     /// Size of the memory region.
//     memory_size: u64,
//     /// HVA.用户空间地址。它是对应的用户空间地址，用于与来宾物理地址进行映射
//     userspace_addr: u64,
//     /// No flags specified for now. 填充字段，通常用于对齐或将来扩展使用。
//     flags_padding: u64,
// }
//
// #[derive(Clone)]
// pub struct VhostMemoryInfo {
//     regions: Arc<Mutex<Vec<VhostMemoryRegion>>>,
//     enabled: bool
// }
//
// impl VhostMemoryInfo {
//     fn new() -> VhostMemoryInfo {
//         VhostMemoryInfo {
//             regions: Arc::new(Mutex::new(Vec::new())),
//             enabled: false,
//         }
//     }
//
//     fn addr_to_host(&self, addr: GuestAddress) -> Option<u64> {
//         let addr = addr.raw_value();
//         for region in self.regions.lock().unwrap().iter() {
//             if addr >= region.guest_phys_addr && addr < region.guest_phys_addr + region.memory_size
//             {
//                 let offset = addr - region.guest_phys_addr;
//                 return Some(region.userspace_addr + offset);
//             }
//         }
//         None
//     }
//
//     // fn check_vhost_mem_range(fr: &FlatRange) -> bool {
//     //     fr.owner.region_type() == RegionType::Ram
//     // }
//
//     // fn add_mem_range(&self, fr: &FlatRange) {
//     //     let guest_phys_addr = fr.addr_range.base.raw_value();
//     //     let memory_size = fr.addr_range.size;
//     //     let userspace_addr = fr.owner.get_host_address().unwrap() + fr.offset_in_region;
//     //
//     //     self.regions.lock().unwrap().push(VhostMemoryRegion {
//     //         guest_phys_addr,
//     //         memory_size,
//     //         userspace_addr,
//     //         flags_padding: 0_u64,
//     //     });
//     // }
//
//     // fn delete_mem_range(&self, fr: &FlatRange) {
//     //     let mut mem_regions = self.regions.lock().unwrap();
//     //     let target = VhostMemoryRegion {
//     //         guest_phys_addr: fr.addr_range.base.raw_value(),
//     //         memory_size: fr.addr_range.size,
//     //         userspace_addr: fr.owner.get_host_address().unwrap() + fr.offset_in_region,
//     //         flags_padding: 0_u64,
//     //     };
//     //     for (index, mr) in mem_regions.iter().enumerate() {
//     //         if mr.guest_phys_addr == target.guest_phys_addr
//     //             && mr.memory_size == target.memory_size
//     //             && mr.userspace_addr == target.userspace_addr
//     //             && mr.flags_padding == target.flags_padding
//     //         {
//     //             mem_regions.remove(index);
//     //             return;
//     //         }
//     //     }
//     //     debug!("Vhost: deleting mem region failed: not matched");
//     // }
//
// }
//
// pub struct VhostBackend {
//     fd: File,
//     mem_info: Arc<Mutex<VhostMemoryInfo>>
// }
//
// impl VhostBackend {
//     pub fn new(
//         mem_space: &Arc<AddressSpace>,
//         path: &str,
//         rawfd: Option<RawFd>,
//     ) -> Result<VhostBackend, VhostNetError> {
//         let fd = match rawfd {
//             Some(rawfd) => unsafe { File::from_raw_fd(rawfd) },
//             None => OpenOptions::new()
//                 .read(true)
//                 .write(true)
//                 .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
//                 .open(path)?
//                 // .with_context(|| format!("Failed to open {} for vhost backend.", path))?,
//         };
//         let mem_info = Arc::new(Mutex::new(VhostMemoryInfo::new()));
//         mem_space.register_listener(mem_info.clone())?;
//
//         Ok(VhostBackend { fd, mem_info })
//     }
//
//     fn set_backend(&self, queue_index: usize, fd: RawFd) -> Result<()> {
//         let vring_file = VhostVringFile {
//             index: queue_index as u32,
//             fd,
//         };
//
//         let ret = unsafe { ioctl_with_ref(self, VHOST_NET_SET_BACKEND(), &vring_file) };
//         if ret < 0 {
//             // return Err(anyhow!(VirtioError::VhostIoctl(
//             //     "VHOST_NET_SET_BACKEND".to_string()
//             // )));
//         }
//         Ok(())
//     }
// }
//
// impl AsRawFd for VhostBackend {
//     fn as_raw_fd(&self) -> RawFd {
//         self.fd.as_raw_fd()
//     }
// }
//
// impl VhostOps for VhostBackend {
//     fn set_owner(&self) -> Result<()> {
//         let ret = unsafe { ioctl(self, VHOST_SET_OWNER()) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_OWNER".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn reset_owner(&self) -> Result<()> {
//         let ret = unsafe { ioctl(self, VHOST_RESET_OWNER()) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_RESET_OWNER".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn get_features(&self) -> Result<u64> {
//         let mut avail_features: u64 = 0;
//         let ret = unsafe { ioctl_with_mut_ref(self, VHOST_GET_FEATURES(), &mut avail_features) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_GET_FEATURES".to_string()
//             )));
//         }
//         Ok(avail_features)
//     }
//
//     fn set_features(&self, features: u64) -> Result<()> {
//         let ret = unsafe { ioctl_with_ref(self, VHOST_SET_FEATURES(), &features) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_FEATURES".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn set_mem_table(&self) -> Result<()> {
//         let regions = self.mem_info.lock().unwrap().regions.lock().unwrap().len();
//         let vm_size = std::mem::size_of::<VhostMemory>();
//         let vmr_size = std::mem::size_of::<VhostMemoryRegion>();
//         let mut bytes: Vec<u8> = vec![0; vm_size + regions * vmr_size];
//
//         bytes[0..vm_size].copy_from_slice(
//             VhostMemory {
//                 nregions: regions as u32,
//                 padding: 0,
//             }
//                 .as_bytes(),
//         );
//
//         let locked_mem_info = self.mem_info.lock().unwrap();
//         let locked_regions = locked_mem_info.regions.lock().unwrap();
//         for (index, region) in locked_regions.iter().enumerate() {
//             bytes[(vm_size + index * vmr_size)..(vm_size + (index + 1) * vmr_size)]
//                 .copy_from_slice(region.as_bytes());
//         }
//
//         let ret = unsafe { ioctl_with_ptr(self, VHOST_SET_MEM_TABLE(), bytes.as_ptr()) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_MEM_TABLE".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn set_vring_num(&self, queue_idx: usize, num: u16) -> Result<()> {
//         let vring_state = VhostVringState {
//             index: queue_idx as u32,
//             num: u32::from(num),
//         };
//         let ret = unsafe { ioctl_with_ref(self, VHOST_SET_VRING_NUM(), &vring_state) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_VRING_NUM".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn set_vring_addr(&self, queue_config: &QueueConfig, index: usize, flags: u32) -> Result<()> {
//         let locked_mem_info = self.mem_info.lock().unwrap();
//         let desc_user_addr = locked_mem_info
//             .addr_to_host(queue_config.desc_table)
//             .with_context(|| {
//                 format!(
//                     "Failed to transform desc-table address {}",
//                     queue_config.desc_table.0
//                 )
//             })?;
//         let used_user_addr = locked_mem_info
//             .addr_to_host(queue_config.used_ring)
//             .with_context(|| {
//                 format!(
//                     "Failed to transform used ring address {}",
//                     queue_config.used_ring.0
//                 )
//             })?;
//         let avail_user_addr = locked_mem_info
//             .addr_to_host(queue_config.avail_ring)
//             .with_context(|| {
//                 format!(
//                     "Failed to transform avail ring address {}",
//                     queue_config.avail_ring.0
//                 )
//             })?;
//
//         let vring_addr = VhostVringAddr {
//             index: index as u32,
//             flags,
//             desc_user_addr,
//             used_user_addr,
//             avail_user_addr,
//             log_guest_addr: 0_u64,
//         };
//
//         let ret = unsafe { ioctl_with_ref(self, VHOST_SET_VRING_ADDR(), &vring_addr) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_VRING_ADDR".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn set_vring_base(&self, queue_idx: usize, num: u16) -> Result<()> {
//         let vring_state = VhostVringState {
//             index: queue_idx as u32,
//             num: u32::from(num),
//         };
//         let ret = unsafe { ioctl_with_ref(self, VHOST_SET_VRING_BASE(), &vring_state) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_VRING_BASE".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn get_vring_base(&self, queue_idx: usize) -> Result<u16> {
//         let vring_state = VhostVringState {
//             index: queue_idx as u32,
//             num: 0,
//         };
//
//         let ret = unsafe { ioctl_with_ref(self, VHOST_GET_VRING_BASE(), &vring_state) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_GET_VRING_BASE".to_string()
//             )));
//         }
//         Ok(vring_state.num as u16)
//     }
//
//     fn set_vring_call(&self, queue_idx: usize, fd: Arc<EventFd>) -> Result<()> {
//         let vring_file = VhostVringFile {
//             index: queue_idx as u32,
//             fd: fd.as_raw_fd(),
//         };
//         let ret = unsafe { ioctl_with_ref(self, VHOST_SET_VRING_CALL(), &vring_file) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_VRING_CALL".to_string()
//             )));
//         }
//         Ok(())
//     }
//
//     fn set_vring_kick(&self, queue_idx: usize, fd: Arc<EventFd>) -> Result<()> {
//         let vring_file = VhostVringFile {
//             index: queue_idx as u32,
//             fd: fd.as_raw_fd(),
//         };
//         let ret = unsafe { ioctl_with_ref(self, VHOST_SET_VRING_KICK(), &vring_file) };
//         if ret < 0 {
//             return Err(anyhow!(VirtioError::VhostIoctl(
//                 "VHOST_SET_VRING_KICK".to_string()
//             )));
//         }
//         Ok(())
//     }
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkInterfaceConfig {
    pub id: String,
    pub host_dev_name: String,
    pub mac: Option<String>,
    pub tap_fds: Option<Vec<String>>,
    pub vhost_type: Option<String>,
    pub vhost_fds: Option<Vec<i32>>,
    pub iothread: Option<String>,
    pub queues: u16,
    pub mq: bool,
    pub socket_path: Option<String>,
    /// All queues of a net device have the same queue size now.
    pub queue_size: u16,
}

impl Default for NetworkInterfaceConfig {
    fn default() -> Self {
        NetworkInterfaceConfig {
            id: "".to_string(),
            host_dev_name: "".to_string(),
            mac: None,
            tap_fds: None,
            vhost_type: None,
            vhost_fds: None,
            iothread: None,
            queues: 2,
            mq: false,
            socket_path: None,
            queue_size: 2,
        }
    }
}

#[derive(Clone)]
pub struct VhostKernHandleImpl<T: VhostKernHandleBackend> {
    pub vk: T,
    pub socket_path: String,
}
pub struct VhostKernNetImpl<T: VhostKernHandleBackend> {
    taps: Vec<Tap>,

    pub avail_features: u64,
    pub acked_features: u64,
    config: NetworkInterfaceConfig,
    pub activate_evt: EventFd,

    queue_sizes: Arc<Vec<u16>>,
    ctrl_queue_size: u16,
    state: DeviceState,
    pub irq_trigger: IrqTrigger,

    kernel_vring_bases: Option<Vec<(u32, u32)>>,
    pub vk_handle: VhostKernHandleImpl<T>,
}

impl<T: VhostKernHandleBackend> VhostKernNetImpl<T> {
    pub fn new(cfg: &NetworkInterfaceConfig) -> Self {
        Self {
            taps: vec![],
            avail_features: 0,
            acked_features: 0,

            config: Default::default(),
            activate_evt: (),
            queue_sizes: Arc::new(vec![]),
            ctrl_queue_size: 0,
            state: DeviceState::Inactive,
            irq_trigger: IrqTrigger {},
            kernel_vring_bases: None,
            vk_handle: VhostKernHandleImpl {},
        }
    }
}

impl VirtioDevice for Net {
    /// Realize vhost virtio network device.
    fn realize(&mut self) -> Result<()> {
        let queue_pairs = self.net_cfg.queues / 2;
        let mut backends = Vec::with_capacity(queue_pairs as usize);
        for index in 0..queue_pairs {
            let fd = if let Some(fds) = self.net_cfg.vhost_fds.as_mut() {
                fds.get(index as usize).copied()
            } else {
                None
            };

            let backend = VhostBackend::new(&self.mem_space, "/dev/vhost-net", fd)
                .with_context(|| "Failed to create backend for vhost net")?;
            backend
                .set_owner()
                .with_context(|| "Failed to set owner for vhost net")?;
            backends.push(backend);
        }

        let mut vhost_features = backends[0]
            .get_features()
            .with_context(|| "Failed to get features for vhost net")?;
        vhost_features &= !(1_u64 << VHOST_NET_F_VIRTIO_NET_HDR);
        vhost_features &= !(1_u64 << VIRTIO_F_ACCESS_PLATFORM);

        let mut device_features = vhost_features;
        device_features |= 1 << VIRTIO_F_VERSION_1
            | 1 << VIRTIO_NET_F_CSUM
            | 1 << VIRTIO_NET_F_GUEST_CSUM
            | 1 << VIRTIO_NET_F_GUEST_TSO4
            | 1 << VIRTIO_NET_F_GUEST_UFO
            | 1 << VIRTIO_NET_F_HOST_TSO4
            | 1 << VIRTIO_NET_F_HOST_UFO;

        let mut locked_state = self.state.lock().unwrap();
        if self.net_cfg.mq
            && (VIRTIO_NET_CTRL_MQ_VQ_PAIRS_MIN..=VIRTIO_NET_CTRL_MQ_VQ_PAIRS_MAX)
            .contains(&queue_pairs)
        {
            device_features |= 1 << VIRTIO_NET_F_CTRL_VQ;
            device_features |= 1 << VIRTIO_NET_F_MQ;
            locked_state.config_space.max_virtqueue_pairs = queue_pairs;
        }

        if let Some(mac) = &self.net_cfg.mac {
            device_features |= build_device_config_space(&mut locked_state.config_space, mac);
        }

        let host_dev_name = match self.net_cfg.host_dev_name.as_str() {
            "" => None,
            _ => Some(self.net_cfg.host_dev_name.as_str()),
        };

        self.taps = create_tap(self.net_cfg.tap_fds.as_ref(), host_dev_name, queue_pairs)
            .with_context(|| "Failed to create tap for vhost net")?;
        self.backends = Some(backends);
        locked_state.device_features = device_features;
        self.vhost_features = vhost_features;

        Ok(())
    }

    fn unrealize(&mut self) -> Result<()> {
        Ok(())
    }

    /// Get the virtio device type, refer to Virtio Spec.
    fn device_type(&self) -> u32 {
        VIRTIO_TYPE_NET
    }

    /// Get the count of virtio device queues.
    fn queue_num(&self) -> usize {
        if self.net_cfg.mq {
            (self.net_cfg.queues + 1) as usize
        } else {
            QUEUE_NUM_NET
        }
    }

    /// Get the queue size of virtio device.
    fn queue_size(&self) -> u16 {
        self.net_cfg.queue_size
    }

    /// Get device features from host.
    fn get_device_features(&self, features_select: u32) -> u32 {
        read_u32(self.state.lock().unwrap().device_features, features_select)
    }

    /// Set driver features by guest.
    fn set_driver_features(&mut self, page: u32, value: u32) {
        self.state.lock().unwrap().driver_features = self.checked_driver_features(page, value);
    }

    /// Get driver features by guest.
    fn get_driver_features(&self, features_select: u32) -> u32 {
        read_u32(self.state.lock().unwrap().driver_features, features_select)
    }

    /// Read data of config from guest.
    fn read_config(&self, offset: u64, mut data: &mut [u8]) -> Result<()> {
        let locked_state = self.state.lock().unwrap();
        let config_slice = locked_state.config_space.as_bytes();
        let config_size = config_slice.len() as u64;
        if offset >= config_size {
            return Err(anyhow!(VirtioError::DevConfigOverflow(offset, config_size)));
        }
        if let Some(end) = offset.checked_add(data.len() as u64) {
            data.write_all(&config_slice[offset as usize..cmp::min(end, config_size) as usize])?;
        }

        Ok(())
    }

    /// Write data to config from guest.
    fn write_config(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let data_len = data.len();
        let mut locked_state = self.state.lock().unwrap();
        let driver_features = locked_state.driver_features;
        let config_slice = locked_state.config_space.as_mut_bytes();

        if !virtio_has_feature(driver_features, VIRTIO_NET_F_CTRL_MAC_ADDR)
            && !virtio_has_feature(driver_features, VIRTIO_F_VERSION_1)
            && offset == 0
            && data_len == MAC_ADDR_LEN
            && *data != config_slice[0..data_len]
        {
            config_slice[(offset as usize)..(offset as usize + data_len)].copy_from_slice(data);
        }

        Ok(())
    }

    fn set_guest_notifiers(&mut self, queue_evts: &[Arc<EventFd>]) -> Result<()> {
        if self.disable_irqfd {
            return Err(anyhow!("The irqfd cannot be used on the current machine."));
        }

        for fd in queue_evts.iter() {
            self.call_events.push(fd.clone());
        }

        Ok(())
    }

    /// Activate the virtio device, this function is called by vcpu thread when frontend
    /// virtio driver is ready and write `DRIVER_OK` to backend.
    fn activate(
        &mut self,
        mem_space: Arc<AddressSpace>,
        interrupt_cb: Arc<VirtioInterrupt>,
        queues: &[Arc<Mutex<Queue>>],
        queue_evts: Vec<Arc<EventFd>>,
    ) -> Result<()> {
        let queue_num = queues.len();
        let driver_features = self.state.lock().unwrap().driver_features;
        if (driver_features & 1 << VIRTIO_NET_F_CTRL_VQ != 0) && (queue_num % 2 != 0) {
            let ctrl_queue = queues[queue_num - 1].clone();
            let ctrl_queue_evt = queue_evts[queue_num - 1].clone();
            let ctrl_info = Arc::new(Mutex::new(CtrlInfo::new(self.state.clone())));

            let ctrl_handler = NetCtrlHandler {
                ctrl: CtrlVirtio::new(ctrl_queue, ctrl_queue_evt, ctrl_info),
                mem_space,
                interrupt_cb: interrupt_cb.clone(),
                driver_features,
                device_broken: self.broken.clone(),
            };

            let notifiers =
                EventNotifierHelper::internal_notifiers(Arc::new(Mutex::new(ctrl_handler)));
            register_event_helper(
                notifiers,
                self.net_cfg.iothread.as_ref(),
                &mut self.deactivate_evts,
            )?;
        }

        let queue_pairs = queue_num / 2;
        for index in 0..queue_pairs {
            let mut host_notifies = Vec::new();
            let backend = match &self.backends {
                None => return Err(anyhow!("Failed to get backend for vhost net")),
                Some(backends) => backends
                    .get(index)
                    .with_context(|| format!("Failed to get index {} vhost backend", index))?,
            };

            backend
                .set_features(self.vhost_features)
                .with_context(|| "Failed to set features for vhost net")?;
            backend
                .set_mem_table()
                .with_context(|| "Failed to set mem table for vhost net")?;

            for queue_index in 0..2 {
                let queue_mutex = queues[index * 2 + queue_index].clone();
                let queue = queue_mutex.lock().unwrap();
                let actual_size = queue.vring.actual_size();
                let queue_config = queue.vring.get_queue_config();

                backend
                    .set_vring_num(queue_index, actual_size)
                    .with_context(|| {
                        format!(
                            "Failed to set vring num for vhost net, index: {} size: {}",
                            queue_index, actual_size,
                        )
                    })?;
                backend
                    .set_vring_addr(&queue_config, queue_index, 0)
                    .with_context(|| {
                        format!(
                            "Failed to set vring addr for vhost net, index: {}",
                            queue_index,
                        )
                    })?;
                backend.set_vring_base(queue_index, 0).with_context(|| {
                    format!(
                        "Failed to set vring base for vhost net, index: {}",
                        queue_index,
                    )
                })?;
                backend
                    .set_vring_kick(queue_index, queue_evts[index * 2 + queue_index].clone())
                    .with_context(|| {
                        format!(
                            "Failed to set vring kick for vhost net, index: {}",
                            index * 2 + queue_index,
                        )
                    })?;

                drop(queue);

                let event = if self.disable_irqfd {
                    let host_notify = VhostNotify {
                        notify_evt: Arc::new(
                            EventFd::new(libc::EFD_NONBLOCK)
                                .with_context(|| VirtioError::EventFdCreate)?,
                        ),
                        queue: queue_mutex.clone(),
                    };
                    let event = host_notify.notify_evt.clone();
                    host_notifies.push(host_notify);
                    event
                } else {
                    self.call_events[queue_index].clone()
                };
                backend
                    .set_vring_call(queue_index, event)
                    .with_context(|| {
                        format!(
                            "Failed to set vring call for vhost net, index: {}",
                            queue_index,
                        )
                    })?;

                let tap = match &self.taps {
                    None => bail!("Failed to get tap for vhost net"),
                    Some(taps) => taps[index].clone(),
                };
                backend
                    .set_backend(queue_index, tap.file.as_raw_fd())
                    .with_context(|| {
                        format!(
                            "Failed to set tap device for vhost net, index: {}",
                            queue_index,
                        )
                    })?;
            }

            if self.disable_irqfd {
                let handler = VhostIoHandler {
                    interrupt_cb: interrupt_cb.clone(),
                    host_notifies,
                    device_broken: self.broken.clone(),
                };
                let notifiers =
                    EventNotifierHelper::internal_notifiers(Arc::new(Mutex::new(handler)));
                register_event_helper(
                    notifiers,
                    self.net_cfg.iothread.as_ref(),
                    &mut self.deactivate_evts,
                )?;
            }
        }
        self.broken.store(false, Ordering::SeqCst);

        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        unregister_event_helper(self.net_cfg.iothread.as_ref(), &mut self.deactivate_evts)?;
        if !self.disable_irqfd {
            self.call_events.clear();
        }

        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        let queue_pairs = self.net_cfg.queues / 2;
        for index in 0..queue_pairs as usize {
            let backend = match &self.backends {
                None => return Err(anyhow!("Failed to get backend for vhost net")),
                Some(backends) => backends
                    .get(index)
                    .with_context(|| format!("Failed to get index {} vhost backend", index))?,
            };

            // 2 queues: rx and tx.
            for queue_index in 0..2 {
                backend.set_backend(queue_index, -1)?;
            }
        }

        Ok(())
    }

    fn get_device_broken(&self) -> &Arc<AtomicBool> {
        &self.broken
    }
}
