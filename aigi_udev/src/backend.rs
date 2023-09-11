use std::{
    collections::HashMap,
    os::fd::FromRawFd,
    path::{Path, PathBuf},
};

use super::LoopData;

use smithay::{
    backend::{
        allocator::{
            gbm::GbmDevice,
            gbm::{GbmAllocator, GbmBufferFlags},
            Fourcc,
        },
        drm::{DrmDevice, DrmDeviceFd, DrmNode, GbmBufferedSurface, NodeType},
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
    wayland::dmabuf::DmabufState,
};
use smithay_drm_extras::drm_scanner::DrmScanner;

// we cannot simply pick the first supported format of the intersection of *all* formats, because
// - we do not want something like Abgr4444, which looses color information, if something better is available
// - some formats might perform terribly
// - we might need some work-arounds, if one supports modifiers, but the other does not
//
// So lets just pick `ARGB2101010` (10-bit) or `ARGB8888` (8-bit) for now, they are widely supported.
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

pub struct BackendData {
    session: LibSeatSession,
    device_data: DeviceData,
    // primary_gpu: DrmNode, // I will not use it, it seems useless
    gpu_manager: GpuManager<GbmGlesBackend<GlesRenderer>>,
    // Alloctor SEEMS to be needed only for multiple GPU systems
    // allocator: Option<Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>>,
}

pub struct DeviceData {
    drm: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    // A single surface is handled
    // surfaces: HashMap<crtc::Handle, ?SurfaceData?>,
    gbm_surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    // drm_scanner: DrmScanner, not saved because no real time update is managed
    render_node: DrmNode,
    // This is used to save the token related to
    // the callback inserted in the event Loop to manage VBlank events!
    //registration_token: RegistrationToken,
}

impl BackendData {
    // This function should prepare ALL the backend
    // and:
    // - Insert in the event loop everything related to the backend managment
    pub fn init(
        event_loop: &mut EventLoop<LoopData>,
        // display: &mut Display<Self>,
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
                    x.clone(),
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

        let (gpu_manager, device_data) =
            Self::init_device(&mut session, primary_gpu_path, primary_gpu_node)?;

        Ok(BackendData {
            session,
            gpu_manager,
            device_data,
        })
    }

    fn init_device(
        session: &mut LibSeatSession,
        path: PathBuf,
        node: DrmNode,
    ) -> Result<
        (
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
        let gbm_allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );

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
        // just take the first connected connector and crtc
        let (connector, crtc) = match scan_results
            .connected
            .iter()
            .next()
            .ok_or("No Connectors available")?
        {
            (connector, Some(crtc)) => (connector, crtc),
            _ => return Err("No available crtc".into()),
        };

        // Monitors have diferent modes that can be selected, eg. 1080x1920@90hz
        // let's choose the preferred one
        let mode_id = connector
            .modes()
            .iter()
            .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(0);

        let drm_mode = connector.modes()[mode_id];

        // Createa a surface that can be used to render stuff
        let drm_surface = drm.create_surface(*crtc, drm_mode, &[connector.handle()])?;

        // TODO: inside Anvil while preparing the connector also all the
        // things realted to AnvilState are prepared (like the Output or the mapping
        // of the Output in the Space) -> I preperf to SPLIT the things and doing that later
        // in a separed function, here I just what to initialized all the backend stuff
        //
        // maybe the output name should be prepared here
        // let output_name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());

        // I will NOT use the DRM Compositor with different Planes for NOW
        // An update of the project could involve the addition of multiple planes
        // For now Only a surface Will be scanout to the screen (the gbm_surface)
        // TODO

        let mut renderer = gpu_manager.single_renderer(&render_node)?;
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .clone();

        let mut gbm_surface = GbmBufferedSurface::new(
            drm_surface,
            gbm_allocator.clone(),
            SUPPORTED_FORMATS,
            render_formats,
        )?;

        let device_data = DeviceData {
            drm,
            gbm,
            gbm_surface,
            render_node,
        };

        Ok((gpu_manager, device_data))
    }

    // This method should MAYBE render the frame
    pub fn render_frame(&mut self) {
        todo!()
    }
}
