#![allow(missing_docs)]

use super::{Mode, Output};
use crate::utils::{Logical, Physical, Point, Size, Transform};
use std::{
    convert::{TryFrom, TryInto},
    sync::{Arc, Mutex},
};
use wayland_protocols::wlr::unstable::output_management::v1::{
    client::zwlr_output_head_v1,
    server::{
        zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
        zwlr_output_configuration_v1,
        zwlr_output_head_v1::ZwlrOutputHeadV1,
        zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
        zwlr_output_mode_v1::ZwlrOutputModeV1,
    },
};
use wayland_server::{protocol::wl_output::WlOutput, Client, DispatchData, Display, Filter, Global, Main};

#[derive(Debug)]
struct Inner {
    enabled: bool,
    global: Option<Global<WlOutput>>,
}

impl Default for Inner {
    fn default() -> Inner {
        Inner {
            enabled: true,
            global: None,
        }
    }
}

#[derive(Debug)]
pub struct WlrOutputHead {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Instance {
    output: Output,
    head: Main<ZwlrOutputHeadV1>,
    modes: Vec<ZwlrOutputModeV1>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        for mode in self.modes.drain(..) {
            mode.finished();
        }
        self.head.finished();
    }
}

#[derive(Debug, Default)]
struct ManagerData {
    instances: Vec<Instance>,
}

#[derive(Debug, Default)]
struct PendingConfiguration {
    serial: u32,
    used: bool,
    heads: Vec<(ZwlrOutputHeadV1, Option<ZwlrOutputConfigurationHeadV1>)>,
}

#[derive(Debug, Default, Clone)]
struct PendingOutputConfiguration {
    mode: Option<ModeConfiguration<ZwlrOutputModeV1>>,
    position: Option<Point<i32, Logical>>,
    transform: Option<Transform>,
    scale: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct OutputConfiguration {
    pub mode: Option<ModeConfiguration<Mode>>,
    pub position: Option<Point<i32, Logical>>,
    pub transform: Option<Transform>,
    pub scale: Option<f64>,
}

impl<'a> TryFrom<&'a mut PendingOutputConfiguration> for OutputConfiguration {
    type Error = u32;
    fn try_from(pending: &'a mut PendingOutputConfiguration) -> Result<OutputConfiguration, Self::Error> {
        let mode = match pending.mode.clone() {
            Some(ModeConfiguration::Mode(wlr_mode)) => Some(ModeConfiguration::Mode(
                wlr_mode
                    .as_ref()
                    .user_data()
                    .get::<Mode>()
                    .cloned()
                    .ok_or_else(|| zwlr_output_configuration_head_v1::Error::InvalidMode.to_raw())?,
            )),
            Some(ModeConfiguration::Custom { size, refresh }) => {
                Some(ModeConfiguration::Custom { size, refresh })
            }
            None => None,
        };
        Ok(OutputConfiguration {
            mode,
            position: pending.position,
            transform: pending.transform,
            scale: pending.scale,
        })
    }
}

#[derive(Debug, Clone)]
pub enum ModeConfiguration<M: Clone> {
    Mode(M),
    Custom {
        size: Size<i32, Physical>,
        refresh: Option<i32>,
    },
}

#[derive(Debug)]
pub struct ConfigurationManager {
    inner: Arc<Mutex<ConfigurationManagerInner>>,
}

#[derive(Debug)]
struct ConfigurationManagerInner {
    outputs: Vec<Output>,
    instances: Vec<ZwlrOutputManagerV1>,
    serial_counter: u32,
}

impl ConfigurationManager {
    pub fn add_heads<'a>(&mut self, outputs: impl Iterator<Item = &'a Output>) {
        {
            let mut inner = self.inner.lock().unwrap();
            let new_outputs = outputs.filter(|o| !inner.outputs.contains(o)).collect::<Vec<_>>();

            for output in new_outputs {
                output.user_data().insert_if_missing(|| WlrOutputHead {
                    inner: Arc::new(Mutex::new(Inner::default())),
                });

                inner.outputs.push(output.clone());
            }
        }
    }

    pub fn remove_heads<'a>(&mut self, outputs: impl Iterator<Item = &'a Output>) {
        {
            let mut inner = self.inner.lock().unwrap();
            for output in outputs {
                inner.outputs.retain(|o| o != output);
                if let Some(inner) = output.user_data().get::<WlrOutputHead>() {
                    let mut inner = inner.inner.lock().unwrap();
                    if let Some(global) = inner.global.take() {
                        global.destroy();
                    }
                }
            }
        }
    }

    pub fn update(&mut self, display: &mut Display) {
        let mut inner = self.inner.lock().unwrap();
        inner.instances.retain(|x| x.as_ref().is_alive());
        inner.serial_counter += 1;
        for output in &inner.outputs {
            if let Some(inner) = output.user_data().get::<WlrOutputHead>() {
                let mut inner = inner.inner.lock().unwrap();
                if inner.enabled && inner.global.is_none() {
                    inner.global = Some(output.create_global(display));
                }
                if !inner.enabled && inner.global.is_some() {
                    inner.global.take().unwrap().destroy();
                }
            }
            for manager in inner.instances.iter() {
                send_head_to_manager(manager, output);
            }
        }
        for manager in inner.instances.iter() {
            manager.done(inner.serial_counter);
        }
    }

    pub fn outputs(&self) -> impl Iterator<Item = Output> {
        self.inner.lock().unwrap().outputs.clone().into_iter()
    }
}

pub fn enable_head(output: &Output) {
    output.user_data().insert_if_missing(|| WlrOutputHead {
        inner: Arc::new(Mutex::new(Inner::default())),
    });

    if let Some(inner) = output.user_data().get::<WlrOutputHead>() {
        let mut inner = inner.inner.lock().unwrap();
        inner.enabled = true;
    }
}

pub fn disable_head(output: &Output) {
    output.user_data().insert_if_missing(|| WlrOutputHead {
        inner: Arc::new(Mutex::new(Inner::default())),
    });

    if let Some(inner) = output.user_data().get::<WlrOutputHead>() {
        let mut inner = inner.inner.lock().unwrap();
        inner.enabled = false;
    }
}

pub fn init_wlr_output_configuration<F, C, L>(
    display: &mut Display,
    client_filter: F,
    apply_configuration: C,
    _logger: L,
) -> (ConfigurationManager, Global<ZwlrOutputManagerV1>)
where
    F: FnMut(Client) -> bool + 'static,
    C: Fn(Vec<(Output, Option<OutputConfiguration>)>, bool, DispatchData<'_>) -> bool + 'static,
    L: Into<Option<::slog::Logger>>,
{
    let inner = Arc::new(Mutex::new(ConfigurationManagerInner {
        outputs: Vec::new(),
        instances: Vec::new(),
        serial_counter: 0,
    }));
    let configuration_manager = ConfigurationManager { inner: inner.clone() };
    let apply_configuration = Arc::new(apply_configuration);

    let global = Filter::new(
        move |(manager, _version): (Main<ZwlrOutputManagerV1>, u32), _, _| {
            let inner_clone = inner.clone();
            let apply_configuration = apply_configuration.clone();
            manager
                .as_ref()
                .user_data()
                .set(|| Mutex::new(ManagerData::default()));
            manager.quick_assign(move |manager, req, _| match req {
                zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                    {
                        let inner = inner_clone.lock().unwrap();
                        if serial != inner.serial_counter {
                            id.cancelled();
                        }
                    }

                    id.as_ref().user_data().set(|| {
                        Mutex::new(PendingConfiguration {
                            serial,
                            used: false,
                            heads: Vec::new(),
                        })
                    });

                    let apply_configuration = apply_configuration.clone();
                    let inner_clone = inner_clone.clone();
                    id.quick_assign(move |conf, req, ddata| match req {
                        zwlr_output_configuration_v1::Request::EnableHead { id, head } => {
                            let mut pending = conf
                                .as_ref()
                                .user_data()
                                .get::<Mutex<PendingConfiguration>>()
                                .unwrap()
                                .lock()
                                .unwrap();
                            if pending.heads.iter().any(|(h, _)| *h == head) {
                                head.as_ref().post_error(
                                    zwlr_output_configuration_v1::Error::AlreadyConfiguredHead.to_raw(),
                                    format!("{:?} was already configured", head),
                                );
                                return;
                            }
                            pending.heads.push((head, Some((&*id).clone())));
                            id.as_ref()
                                .user_data()
                                .set(|| Mutex::new(PendingOutputConfiguration::default()));
                            id.quick_assign(move |conf_head, req, _| {
                                let mut pending = conf_head
                                    .as_ref()
                                    .user_data()
                                    .get::<Mutex<PendingOutputConfiguration>>()
                                    .unwrap()
                                    .lock()
                                    .unwrap();

                                match req {
                                    zwlr_output_configuration_head_v1::Request::SetMode { mode } => {
                                        if pending.mode.is_some() {
                                            conf_head.as_ref().post_error(
                                                zwlr_output_configuration_head_v1::Error::AlreadySet.to_raw(),
                                                format!("{:?} already had a mode configured", conf_head),
                                            );
                                            return;
                                        }

                                        pending.mode = Some(ModeConfiguration::Mode(mode));
                                    }
                                    zwlr_output_configuration_head_v1::Request::SetCustomMode {
                                        width,
                                        height,
                                        refresh,
                                    } => {
                                        if pending.mode.is_some() {
                                            conf_head.as_ref().post_error(
                                                zwlr_output_configuration_head_v1::Error::AlreadySet.to_raw(),
                                                format!("{:?} already had a mode configured", conf_head),
                                            );
                                            return;
                                        }

                                        pending.mode = Some(ModeConfiguration::Custom {
                                            size: Size::from((width, height)),
                                            refresh: if refresh == 0 { None } else { Some(refresh) },
                                        });
                                    }
                                    zwlr_output_configuration_head_v1::Request::SetPosition { x, y } => {
                                        if pending.position.is_some() {
                                            conf_head.as_ref().post_error(
                                                zwlr_output_configuration_head_v1::Error::AlreadySet.to_raw(),
                                                format!("{:?} already had a position configured", conf_head),
                                            );
                                            return;
                                        }

                                        pending.position = Some(Point::from((x, y)));
                                    }
                                    zwlr_output_configuration_head_v1::Request::SetScale { scale } => {
                                        if pending.scale.is_some() {
                                            conf_head.as_ref().post_error(
                                                zwlr_output_configuration_head_v1::Error::AlreadySet.to_raw(),
                                                format!("{:?} already had a scale configured", conf_head),
                                            );
                                            return;
                                        }

                                        pending.scale = Some(scale);
                                    }
                                    zwlr_output_configuration_head_v1::Request::SetTransform {
                                        transform,
                                    } => {
                                        if pending.transform.is_some() {
                                            conf_head.as_ref().post_error(
                                                zwlr_output_configuration_head_v1::Error::AlreadySet.to_raw(),
                                                format!("{:?} already had a transform configured", conf_head),
                                            );
                                            return;
                                        }

                                        pending.transform = Some(transform.into());
                                    }
                                    _ => {
                                        // TODO unsupported event
                                    }
                                }
                            })
                        }
                        zwlr_output_configuration_v1::Request::DisableHead { head } => {
                            let mut pending = conf
                                .as_ref()
                                .user_data()
                                .get::<Mutex<PendingConfiguration>>()
                                .unwrap()
                                .lock()
                                .unwrap();
                            if pending.heads.iter().any(|(h, _)| *h == head) {
                                head.as_ref().post_error(
                                    zwlr_output_configuration_v1::Error::AlreadyConfiguredHead.to_raw(),
                                    format!("{:?} was already configured", head),
                                );
                                return;
                            }
                            pending.heads.push((head, None));
                        }
                        x @ zwlr_output_configuration_v1::Request::Apply
                        | x @ zwlr_output_configuration_v1::Request::Test => {
                            let final_conf = {
                                let inner = inner_clone.lock().unwrap();
                                let mut pending = conf
                                    .as_ref()
                                    .user_data()
                                    .get::<Mutex<PendingConfiguration>>()
                                    .unwrap()
                                    .lock()
                                    .unwrap();
                                if pending.used {
                                    return conf.as_ref().post_error(
                                        zwlr_output_configuration_v1::Error::AlreadyUsed.to_raw(),
                                        "Configuration object was used already".to_string(),
                                    );
                                }
                                pending.used = true;
                                if pending.serial != inner.serial_counter {
                                    conf.cancelled();
                                    return;
                                }

                                match pending
                                    .heads
                                    .iter_mut()
                                    .map(|(head, conf)| {
                                        let output = match {
                                            let data = manager
                                                .as_ref()
                                                .user_data()
                                                .get::<Mutex<ManagerData>>()
                                                .unwrap()
                                                .lock()
                                                .unwrap();
                                            data.instances
                                                .iter()
                                                .find(|instance| *instance.head == *head)
                                                .map(|i| i.output.clone())
                                        } {
                                            Some(o) => o,
                                            None => {
                                                return Err(
                                                    zwlr_output_configuration_head_v1::Error::InvalidMode
                                                        .to_raw(),
                                                );
                                            }
                                        };

                                        match conf {
                                            Some(head) => (&mut *head
                                                .as_ref()
                                                .user_data()
                                                .get::<Mutex<PendingOutputConfiguration>>()
                                                .unwrap()
                                                .lock()
                                                .unwrap())
                                                .try_into()
                                                .map(|c| (output, Some(c))),
                                            None => Ok((output, None)),
                                        }
                                    })
                                    .collect::<Result<Vec<(Output, Option<OutputConfiguration>)>, u32>>()
                                {
                                    Ok(conf) => conf,
                                    Err(code) => {
                                        return conf
                                            .as_ref()
                                            .post_error(code, "Incomplete configuration".to_string());
                                    }
                                }
                            };

                            let test_only = matches!(x, zwlr_output_configuration_v1::Request::Test);
                            if apply_configuration(final_conf, test_only, ddata) {
                                conf.succeeded()
                            } else {
                                conf.failed()
                            }
                        }
                        zwlr_output_configuration_v1::Request::Destroy => {
                            // Nothing to do
                        }
                        _ => {
                            // TODO unsupported event
                        }
                    });
                }
                zwlr_output_manager_v1::Request::Stop => {
                    manager.finished();
                    inner_clone
                        .lock()
                        .unwrap()
                        .instances
                        .retain(|instance| instance != &*manager);
                }
                _ => {
                    // TODO unsupported event
                }
            });

            let mut inner = inner.lock().unwrap();
            inner.instances.push((*manager).clone());
            for output in inner.outputs.iter() {
                send_head_to_manager(&*manager, output);
            }
            manager.done(inner.serial_counter);
        },
    );

    (
        configuration_manager,
        display.create_global_with_filter(2, global, client_filter),
    )
}

fn send_head_to_manager(manager: &ZwlrOutputManagerV1, output: &Output) {
    let mut data = manager
        .as_ref()
        .user_data()
        .get::<Mutex<ManagerData>>()
        .unwrap()
        .lock()
        .unwrap();

    let instance = match data.instances.iter_mut().find(|i| i.output == *output) {
        Some(i) => i,
        None => {
            if let Some(client) = manager.as_ref().client() {
                if let Some(head) = client.create_resource::<ZwlrOutputHeadV1>(manager.as_ref().version()) {
                    head.quick_assign(|_, _, _| {}); //currently has no requests
                    manager.head(&head);
                    data.instances.push(Instance {
                        output: output.clone(),
                        head,
                        modes: Vec::new(),
                    });
                    data.instances.last_mut().unwrap()
                } else {
                    return;
                }
            } else {
                return;
            }
        }
    };

    instance.head.name(output.name());
    instance.head.description(output.description());
    let physical = output.physical_properties();
    if !(physical.size.w == 0 || physical.size.h == 0) {
        instance.head.physical_size(physical.size.w, physical.size.h);
    }

    let inner = output
        .user_data()
        .get::<WlrOutputHead>()
        .unwrap()
        .inner
        .lock()
        .unwrap();

    let modes = &mut instance.modes;
    for output_mode in output.modes().into_iter() {
        if let Some(mode) = if let Some(wlr_mode) = modes
            .iter()
            .find(|mode| *mode.as_ref().user_data().get::<Mode>().unwrap() == output_mode)
        {
            Some(wlr_mode)
        } else if let Some(client) = instance.head.as_ref().client() {
                if let Some(mode) = client.create_resource::<ZwlrOutputModeV1>(manager.as_ref().version()) {
                    instance.head.mode(&*mode);
                    mode.size(output_mode.size.w, output_mode.size.h);
                    mode.refresh(output_mode.refresh);
                    if output.preferred_mode().map(|p| p == output_mode).unwrap_or(false) {
                        mode.preferred();
                    }
                    mode.as_ref().user_data().set(|| output_mode);
                    modes.push((&*mode).clone());
                    modes.last()
                } else {
                    None
                }
        } else {
            None
        } {
            if inner.enabled && output.current_mode().map(|c| c == output_mode).unwrap_or(false) {
                instance.head.current_mode(&*mode);
            }
        }
    }

    instance.head.enabled(if inner.enabled { 1 } else { 0 });
    if inner.enabled {
        let point = output.current_location();
        instance.head.position(point.x, point.y);
        instance.head.transform(output.current_transform());
        instance
            .head
            .scale(output.current_scale().fractional_scale());
    }

    if manager.as_ref().version() >= zwlr_output_head_v1::EVT_MAKE_SINCE {
        if physical.make != "Unknown" {
            instance.head.make(physical.make.clone());
        }
        if physical.model != "Unknown" {
            instance.head.model(physical.model);
        }
    }
}
