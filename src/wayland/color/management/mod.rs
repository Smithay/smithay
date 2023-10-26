mod protocol;
use crate::{
    output::{Output, WeakOutput},
    utils::{sealed_file::SealedFile, user_data::UserDataMap},
    wayland::compositor::{self, Cacheable},
};

pub use self::protocol::*;
mod dispatch;

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};
use wayland_server::{protocol::wl_surface::WlSurface, Dispatch, DisplayHandle, GlobalDispatch, Weak};
pub use wp_color_manager_v1::{Feature, RenderIntent};

crate::utils::ids::id_gen!(next_img_desc_id, IMG_DESC_ID, IMG_DESC_IDS);

#[derive(Debug)]
pub struct ColorManagementState {
    supported_rendering_intents: HashSet<RenderIntent>,
    supported_features: HashSet<Feature>,
    supported_tf_cicp: HashSet<u32>,
    supported_primaries_cicp: HashSet<u32>,
    known_image_descriptions: HashMap<ImageDescriptionContents, std::sync::Weak<ImageDescriptionInternal>>,
}

pub trait ColorManagementHandler {
    fn color_management_state(&mut self) -> &mut ColorManagementState;
    fn verify_icc(&mut self, icc_data: &[u8]) -> bool;
    fn description_for_output(&self, output: &Output) -> ImageDescription;
    fn preferred_description_for_surface(&self, surface: &WlSurface) -> ImageDescription;
}

#[derive(Debug)]
pub struct ColorManagementOutput {
    description: Mutex<ImageDescription>,
    known_instances: Mutex<Vec<wp_color_management_output_v1::WpColorManagementOutputV1>>,
}

impl ColorManagementOutput {
    fn new(desc: ImageDescription) -> Self {
        ColorManagementOutput {
            description: Mutex::new(desc),
            known_instances: Mutex::new(Vec::new()),
        }
    }

    fn add_instance(&self, instance: wp_color_management_output_v1::WpColorManagementOutputV1) {
        self.known_instances.lock().unwrap().push(instance);
    }

    fn remove_instance(&self, instance: &wp_color_management_output_v1::WpColorManagementOutputV1) {
        self.known_instances.lock().unwrap().retain(|i| i != instance);
    }
}

pub fn get_surface_description(surface: &WlSurface) -> (Option<ImageDescription>, RenderIntent) {
    let data = compositor::with_states(surface, |states| {
        states
            .cached_state
            .current::<ColorManagementSurfaceCachedState>()
            .clone()
    });
    (data.description, data.render_intent)
}

#[derive(Debug, Clone)]
struct ColorManagementSurfaceCachedState {
    description: Option<ImageDescription>,
    render_intent: RenderIntent,
}

impl Default for ColorManagementSurfaceCachedState {
    fn default() -> Self {
        ColorManagementSurfaceCachedState {
            description: None,
            render_intent: RenderIntent::Perceptual,
        }
    }
}

impl Cacheable for ColorManagementSurfaceCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

#[derive(Debug)]
pub struct ColorManagementSurfaceData {
    preferred: Mutex<ImageDescription>,
    known_instances: Mutex<Vec<wp_color_management_surface_v1::WpColorManagementSurfaceV1>>,
}

impl ColorManagementSurfaceData {
    fn new(preferred_desc: ImageDescription) -> Self {
        Self {
            preferred: Mutex::new(preferred_desc),
            known_instances: Mutex::new(Vec::new()),
        }
    }

    fn add_instance(&self, instance: wp_color_management_surface_v1::WpColorManagementSurfaceV1) {
        self.known_instances.lock().unwrap().push(instance);
    }

    fn remove_instance(&self, instance: &wp_color_management_surface_v1::WpColorManagementSurfaceV1) {
        self.known_instances.lock().unwrap().retain(|i| i != instance);
    }
}

#[derive(Debug)]
pub struct ImageDescriptionData {
    get_information: bool,
    info: ImageDescription,
}

#[derive(Debug, Clone)]
pub struct ImageDescription(Arc<ImageDescriptionInternal>);

impl PartialEq for ImageDescription {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl ImageDescription {
    pub fn contents(&self) -> &ImageDescriptionContents {
        &self.0.contents
    }
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }
}

#[derive(Debug)]
pub struct ImageDescriptionInternal {
    id: usize,
    contents: ImageDescriptionContents,
    user_data: UserDataMap,
}

impl Drop for ImageDescriptionInternal {
    fn drop(&mut self) {
        IMG_DESC_IDS.lock().unwrap().remove(&self.id);
    }
}

#[derive(Debug, Clone)]
pub struct IccData {
    data: Vec<u8>,
    file: Arc<Mutex<Option<SealedFile>>>,
}

impl AsRef<[u8]> for IccData {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Debug, Clone)]
pub enum ImageDescriptionContents {
    ICC(IccData),
    Parametric {
        tf: TransferFunction,
        primaries: Primaries,
        target_primaries: Option<ParametricPrimaries>,
        target_luminance: Option<(u32, u32)>,
        max_cll: Option<u32>,
        max_fall: Option<u32>,
    },
}

impl Hash for ImageDescriptionContents {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ImageDescriptionContents::ICC(IccData { data, .. }) => {
                data.hash(state);
            }
            ImageDescriptionContents::Parametric {
                tf,
                primaries,
                target_primaries,
                target_luminance,
                max_cll: max_ccl,
                max_fall,
            } => {
                tf.hash(state);
                primaries.hash(state);
                target_primaries.hash(state);
                target_luminance.hash(state);
                max_ccl.hash(state);
                max_fall.hash(state);
            }
        }
    }
}

impl PartialEq for ImageDescriptionContents {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ImageDescriptionContents::ICC(IccData { data: data1, .. }),
                ImageDescriptionContents::ICC(IccData { data: data2, .. }),
            ) => data1 == data2,
            (
                ImageDescriptionContents::Parametric {
                    tf: tf1,
                    primaries: primaries1,
                    target_primaries: target_primaries1,
                    target_luminance: target_luminance1,
                    max_cll: max_ccl1,
                    max_fall: max_fall1,
                },
                ImageDescriptionContents::Parametric {
                    tf: tf2,
                    primaries: primaries2,
                    target_primaries: target_primaries2,
                    target_luminance: target_luminance2,
                    max_cll: max_ccl2,
                    max_fall: max_fall2,
                },
            ) => {
                tf1 == tf2
                    && primaries1 == primaries2
                    && target_primaries1 == target_primaries2
                    && target_luminance1 == target_luminance2
                    && max_ccl1 == max_ccl2
                    && max_fall1 == max_fall2
            }
            _ => false,
        }
    }
}

impl Eq for ImageDescriptionContents {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferFunction {
    CICP(u32),
    Power(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primaries {
    CICP(u32),
    Parametric(ParametricPrimaries),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParametricPrimaries {
    red: (u32, u32),
    green: (u32, u32),
    blue: (u32, u32),
    white: (u32, u32),
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum DescriptionError {
    #[error("incomplete parameter set")]
    IncompleteSet,
    #[error("invalid combination of parameters")]
    InconsistentSet,
}

impl ColorManagementState {
    pub fn new<D>(
        dh: &DisplayHandle,
        supported_rendering_intents: impl Iterator<Item = RenderIntent>,
        supported_features: impl Iterator<Item = Feature>,
        supported_tf_cicp: impl Iterator<Item = u32>,
        supported_primaries_cicp: impl Iterator<Item = u32>,
    ) -> ColorManagementState
    where
        D: ColorManagementHandler
            + GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
            + Dispatch<wp_color_manager_v1::WpColorManagerV1, (), D>
            + Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, WeakOutput, D>
            + Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, Weak<WlSurface>, D>
            + Dispatch<wp_image_description_v1::WpImageDescriptionV1, (), D>
            + Dispatch<wp_image_description_v1::WpImageDescriptionV1, ImageDescriptionData, D>
            + Dispatch<
                wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
                Mutex<ImageDescriptionIccBuilder>,
                D,
            > + Dispatch<
                wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
                Mutex<ImageDescriptionParametricBuilder>,
                D,
            > + 'static,
    {
        dh.create_global::<D, wp_color_manager_v1::WpColorManagerV1, ()>(1, ());
        ColorManagementState {
            supported_rendering_intents: supported_rendering_intents.collect(),
            supported_features: supported_features.collect(),
            supported_tf_cicp: supported_tf_cicp.collect(),
            supported_primaries_cicp: supported_primaries_cicp.collect(),
            known_image_descriptions: HashMap::new(),
        }
    }

    pub fn build_description<B: TryInto<ImageDescriptionContents, Error = DescriptionError>>(
        &mut self,
        contents: B,
    ) -> Result<ImageDescription, DescriptionError> {
        let contents = contents.try_into()?;
        let desc = match self
            .known_image_descriptions
            .get(&contents)
            .and_then(std::sync::Weak::upgrade)
        {
            Some(desc) => desc,
            None => {
                let desc = Arc::new(ImageDescriptionInternal {
                    id: next_img_desc_id(),
                    contents: contents.clone(),
                    user_data: UserDataMap::new(),
                });
                self.known_image_descriptions
                    .insert(contents, Arc::downgrade(&desc));
                desc
            }
        };

        Ok(ImageDescription(desc))
    }
}

#[derive(Debug, Default)]
pub struct ImageDescriptionIccBuilder {
    data: Option<Vec<u8>>,
}

impl ImageDescriptionIccBuilder {
    pub fn with_data(&mut self, data: impl AsRef<[u8]>) -> bool {
        let result = self.data.is_some();
        self.data = Some(Vec::from(data.as_ref()));
        result
    }

    pub fn with_file(&mut self, mut file: File, offset: usize, len: usize) -> Result<bool, std::io::Error> {
        let result = self.data.is_some();
        file.seek(SeekFrom::Start(offset as u64))?;

        let mut data = Vec::new();
        let mut buf = [0u8; 4096];

        while let Ok(size) = file.read(&mut buf) {
            if data.len() + size >= len {
                data.extend(&buf[0..(len - data.len())]);
                break;
            } else {
                data.extend(&buf);
            }
        }
        self.data = Some(data);

        Ok(result)
    }
}

impl TryInto<ImageDescriptionContents> for ImageDescriptionIccBuilder {
    type Error = DescriptionError;
    fn try_into(self) -> Result<ImageDescriptionContents, Self::Error> {
        if self.data.is_none() {
            return Err(DescriptionError::IncompleteSet);
        }

        Ok(ImageDescriptionContents::ICC(IccData {
            data: self.data.unwrap(),
            file: Arc::new(Mutex::new(None)),
        }))
    }
}

#[derive(Debug, Default)]
pub struct ImageDescriptionParametricBuilder {
    tf: Option<TransferFunction>,
    primaries: Option<Primaries>,
    target_primaries: Option<ParametricPrimaries>,
    target_luminance: Option<(u32, u32)>,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
}

impl ImageDescriptionParametricBuilder {
    pub fn set_tf(&mut self, tf: TransferFunction) -> bool {
        let result = self.tf.is_some();
        self.tf = Some(tf);
        result
    }

    pub fn set_primaries(&mut self, primaries: Primaries) -> bool {
        let result = self.primaries.is_some();
        self.primaries = Some(primaries);
        result
    }

    pub fn set_target_primaries(&mut self, target_primaries: ParametricPrimaries) -> bool {
        let result = self.target_primaries.is_some();
        self.target_primaries = Some(target_primaries);
        result
    }

    pub fn set_target_luminance(&mut self, target_lumninance: (u32, u32)) -> bool {
        let result = self.target_luminance.is_some();
        self.target_luminance = Some(target_lumninance);
        result
    }

    pub fn set_max_cll(&mut self, max_ccl: u32) -> bool {
        let result = self.max_cll.is_some();
        self.max_cll = Some(max_ccl);
        result
    }

    pub fn set_max_fall(&mut self, max_fall: u32) -> bool {
        let result = self.max_fall.is_some();
        self.max_fall = Some(max_fall);
        result
    }
}

impl TryInto<ImageDescriptionContents> for ImageDescriptionParametricBuilder {
    type Error = DescriptionError;
    fn try_into(self) -> Result<ImageDescriptionContents, Self::Error> {
        if (self.target_luminance.is_some() || self.max_cll.is_some() || self.max_fall.is_some())
            && !(self.tf == Some(TransferFunction::CICP(16)) || self.tf == Some(TransferFunction::CICP(18)))
        {
            return Err(DescriptionError::InconsistentSet);
        }

        if self.tf.is_none() || self.primaries.is_none() {
            return Err(DescriptionError::IncompleteSet);
        }

        Ok(ImageDescriptionContents::Parametric {
            tf: self.tf.unwrap(),
            primaries: self.primaries.unwrap(),
            target_primaries: self.target_primaries,
            target_luminance: self.target_luminance,
            max_cll: self.max_cll,
            max_fall: self.max_fall,
        })
    }
}

/// Macro to delegate implementation of the wp color management protocol
#[macro_export]
macro_rules! delegate_color_management {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __WpColorManagerV1 =
            $crate::wayland::color::management::protocol::wp_color_manager_v1::WpColorManagerV1;
        type __WpColorManagementOutputV1 =
            $crate::wayland::color::management::protocol::wp_color_management_output_v1::WpColorManagementOutputV1;
        type __WpColorManagementSurfaceV1 =
            $crate::wayland::color::management::protocol::wp_color_management_surface_v1::WpColorManagementSurfaceV1;
        type __WpImageDescriptionV1 =
            $crate::wayland::color::management::protocol::wp_image_description_v1::WpImageDescriptionV1;
        type __WpImageDescriptionCreatorIccV1 =
            $crate::wayland::color::management::protocol::wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1;
        type __WpImageDescriptionCreatorParamsV1 =
            $crate::wayland::color::management::protocol::wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorManagerV1: ()
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorManagerV1: ()
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorManagementOutputV1: $crate::output::WeakOutput
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpColorManagementSurfaceV1: wayland_server::Weak<wayland_server::protocol::surface::WlSurface>
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpImageDescriptionV1: ()
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpImageDescriptionV1: $crate::wayland::color::management::ImageDescriptionData
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpImageDescriptionCreatorIccV1: std::sync::Mutex<$crate::wayland::color::management::ImageDescriptionIccBuilder>
            ] => $crate::wayland::color::management::ColorManagementState
        );

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty:
            [
                __WpImageDescriptionCreatorParamsV1: std::sync::Mutex<$crate::wayland::color::management::ImageDescriptionParametricBuilder>
            ] => $crate::wayland::color::management::ColorManagementState
        );
    };
}
