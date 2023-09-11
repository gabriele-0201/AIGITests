use std::{
    collections::HashMap,
    os::fd::FromRawFd,
    path::{Path, PathBuf},
};

use super::LoopData;

use smithay::{
    backend::{
        allocator::GbmDevice,
        drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType},
        egl::{EGLDevice, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles::GlesRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager},
        },
        session::{libseat::LibSeatSession, Session},
        udev::{primary_gpu, UdevBackend},
    },
    reexports::{
        calloop::{EventLoop, RegistrationToken},
        drm::control::{crtc, ModeTypeFlags},
        input::Libinput,
        nix::fcntl::OFlag,
        wayland_server::Display,
    },
    utils::DeviceFd,
    wayland::compositor::SurfaceData,
};
use smithay_drm_extras::drm_scanner::DrmScanner;

pub struct BackendData {
    session: LibSeatSession,
    device_data: DeviceData,
}

pub struct DeviceData {
    surfaces: HashMap<crtc::Handle, SurfaceData>,
    gbm: GbmDevice<DrmDeviceFd>,
    drm: DrmDevice,
    // drm_scanner: DrmScanner, not saved because no real time update is managed
    render_node: DrmNode,
    registration_token: RegistrationToken,
}

impl BackendData {
    pub fn init(
        event_loop: &mut EventLoop<LoopData>,
        display: &mut Display<Self>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Initialize session
        // The session_notifier should be insered in the event_loop
        // by the caller of this method
        let (mut session, session_notifier) = LibSeatSession::new()?;

        // Initialize libinput backend
        let mut libinput_context = Libinput::new_with_udev::<
            LibinputSessionInterface<LibSeatSession>,
        >(session.clone().into());
        libinput_context.udev_assign_seat(&session.seat()).unwrap();
        // Handler to be managed by the caller
        let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

        // Search primary GPU and save it in a DrmNode
        // if not found then return Error
        let (primary_gpu_path, primary_gpu_node) = primary_gpu(&session.seat())
            .unwrap()
            .and_then(|x| {
                Some((
                    x,
                    DrmNode::from_path(x)
                        .ok()?
                        .node_with_type(NodeType::Render)?
                        .ok()?,
                ))
            })
            .ok_or_else(|| "Impossible find primary gpu")?;

        // Setup the GPU manager,
        // multiple gpus could be handled BUT for now a single
        // udev_device / gpu is handled (the primary!)
        // (each udev device is a graphics device ?!)

        let (render_node, gpu_manager, device_data) =
            Self::init_device(&session, primary_gpu_path, primary_gpu_node)?;

        // let mut backend_data = BackendData {
        //     devices: HashMap::new(),
        // };

        // Init AigiState ????

        // Initialize the udev backend
        //?? let udev_backend = UdevBackend::new(&session.seat()).unwrap();

        //?? // Scan all the already present devices
        //?? for (device_id, path) in udev_backend.device_list() {
        //??     backend_data.udev_add_device(device_id, path)?;
        //?? }

        todo!()
    }

    fn init_device(
        session: &LibSeatSession,
        path: PathBuf,
        node: DrmNode,
    ) -> Result<
        (
            DrmNode,                                  // Renderer Node
            GpuManager<GbmGlesBackend<GlesRenderer>>, // Gpu Manager
            DeviceData, // All the initialized information about the Device that will render stuff on the screen
        ),
        Box<dyn std::error::Error>,
    > {
        // Try to open the device
        let fd = session.open(
            &path,
            OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK,
        )?;

        // Wrap the file descriptor into a smithay FileDescriptor
        let fd = DrmDeviceFd::new(unsafe { DeviceFd::from_raw_fd(fd) });

        // Now we can initialize the drm device
        let (drm, drm_event_source) = DrmDevice::new(fd, false)?;

        // Creation of the gbm device and the GbmAllocator
        let gbm = GbmDevice::new(drm.device_fd().clone())?;
        //??? let gbm_allocator = GbmAllocator::new(
        //???     gbm.clone(),
        //???     GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        //??? );

        // We also want to aquire the node of a device that will be doing the rendering
        // (On typical desktop it will probably equal the node that DrmDevice was created from,
        // but on some ARM setups this is splited into two separate nodes,
        // one for gpu acceleration and one for handling outputs)
        let render_node = EGLDevice::device_for_display(&EGLDisplay::new(gbm.clone()).unwrap())
            .and_then(|x| x.try_get_render_node())?
            .unwrap_or(node);

        let mut gpu_manager: GpuManager<GbmGlesBackend<GlesRenderer>> =
            GpuManager::new(Default::default())?;
        gpu_manager.as_mut().add_node(render_node, gbm.clone())?;

        let mut drm_scanner: DrmScanner = DrmScanner::default();
        // The following should be called every time Udev::Changed event is fired,
        // to make sure all newly connected outputs are initialized,
        let scan_results = drm_scanner.scan_connectors(&drm);
        let added = scan_results;

        // just take the first connected connector
        if let (connector, Some(crtc)) = scan_results
            .connected
            .iter()
            .next()
            .ok_or("No Connectors available")?
        {
            // Monitors have diferent modes that can be selected, eg. 1080x1920@90hz
            // let's choose the preferred one
            let mode_id = connector
                .modes()
                .iter()
                .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                .unwrap_or(0);

            let drm_mode = connector.modes()[mode_id];
            // TODO
        }

        todo!()
    }

    pub fn udev_add_device(
        &mut self,
        device_id: u64,
        path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let node = DrmNode::from_dev_id(device_id)?;
        todo!()
    }

    pub fn udev_remove_device(
        &mut self,
        device_id: u64,
        path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        todo!()
    }
}
