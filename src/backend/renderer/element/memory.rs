//! TODO: docs

use std::{
    any::TypeId,
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex, MutexGuard},
};

use slog::trace;

use crate::{
    backend::renderer::{
        utils::{CommitCounter, DamageTracker},
        Frame, ImportMem, Renderer,
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Size, Transform},
};

use super::{Id, RenderElement};

#[derive(Debug, Default)]
struct MemoryRenderBufferInner {
    mem: Vec<u8>,
    damage_tracker: DamageTracker<i32, Buffer>,
    textures: HashMap<(TypeId, usize), Box<dyn std::any::Any>>,
    renderer_seen: HashMap<(TypeId, usize), CommitCounter>,
}

impl MemoryRenderBufferInner {
    fn from_memory(mem: &[u8]) -> Self {
        MemoryRenderBufferInner {
            mem: mem.to_vec(),
            damage_tracker: DamageTracker::default(),
            textures: HashMap::default(),
            renderer_seen: HashMap::default(),
        }
    }

    fn resize(&mut self, size: usize) {
        if self.mem.len() != size {
            self.mem.resize(size, 0);
            self.renderer_seen.clear();
            self.textures.clear();
            self.damage_tracker.reset();
        }
    }

    fn import_texture<R>(
        &mut self,
        renderer: &mut R,
        buffer_size: Size<i32, Buffer>,
        log: &slog::Logger,
    ) -> Result<&<R as Renderer>::TextureId, <R as Renderer>::Error>
    where
        R: Renderer + ImportMem,
        <R as Renderer>::TextureId: 'static,
    {
        let texture_id = (TypeId::of::<<R as Renderer>::TextureId>(), renderer.id());
        let current_commit = self.damage_tracker.current_commit();
        let last_commit = self.renderer_seen.get(&texture_id).copied();
        let buffer_damage = self
            .damage_tracker
            .damage_since(last_commit)
            .map(|d| d.into_iter().reduce(|a, b| a.merge(b)).unwrap_or_default())
            .unwrap_or_else(|| Rectangle::from_loc_and_size(Point::default(), buffer_size));

        match self.textures.entry(texture_id) {
            Entry::Occupied(entry) => {
                if !buffer_damage.is_empty() {
                    trace!(log, "updating memory with damage {:#?}", &buffer_damage);
                    renderer.update_memory(entry.get().downcast_ref().unwrap(), &self.mem, buffer_damage)?
                }
            }
            Entry::Vacant(entry) => {
                trace!(log, "importing memory");
                let tex = renderer.import_memory(&self.mem, buffer_size, false)?;
                entry.insert(Box::new(tex));
            }
        };

        self.renderer_seen.insert(texture_id, current_commit);

        Ok(self
            .textures
            .get(&texture_id)
            .unwrap()
            .downcast_ref::<<R as Renderer>::TextureId>()
            .unwrap())
    }
}

/// A memory backed render buffer
#[derive(Debug, Clone)]
pub struct MemoryRenderBuffer {
    id: Id,
    inner: Arc<Mutex<MemoryRenderBufferInner>>,
}

impl Default for MemoryRenderBuffer {
    fn default() -> Self {
        Self {
            id: Id::new(),
            inner: Default::default(),
        }
    }
}

impl MemoryRenderBuffer {
    /// Initialize a [`MemoryRenderBuffer`] from existing memory
    pub fn from_memory(mem: &[u8]) -> Self {
        let inner = MemoryRenderBufferInner::from_memory(mem);
        MemoryRenderBuffer {
            id: Id::new(),
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Render to the memory buffer
    pub fn render(&self) -> RenderContext<'_> {
        let guard = self.inner.lock().unwrap();
        RenderContext {
            buffer: guard,
            damage: Vec::new(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.lock().unwrap().damage_tracker.current_commit()
    }

    fn damage_since(&self, commit: Option<CommitCounter>) -> Option<Vec<Rectangle<i32, Buffer>>> {
        self.inner.lock().unwrap().damage_tracker.damage_since(commit)
    }
}

/// A render context for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct RenderContext<'a> {
    buffer: MutexGuard<'a, MemoryRenderBufferInner>,
    damage: Vec<Rectangle<i32, Buffer>>,
}

impl<'a> RenderContext<'a> {
    /// Resize the buffer
    pub fn resize(&mut self, size: usize) {
        self.buffer.resize(size)
    }

    /// Draw to the buffer
    pub fn draw<F>(&mut self, f: F)
    where
        F: FnOnce(&mut [u8]) -> Vec<Rectangle<i32, Buffer>>,
    {
        let draw_damage = f(&mut self.buffer.mem);
        self.damage.extend(draw_damage);
    }
}

impl<'a> Drop for RenderContext<'a> {
    fn drop(&mut self) {
        self.buffer.damage_tracker.add(&self.damage);
    }
}

/// A render element for [`MemoryRenderBuffer`]
#[derive(Debug)]
pub struct MemoryRenderBufferRenderElement {
    buffer: MemoryRenderBuffer,
    buffer_scale: i32,
    buffer_transform: Transform,
    buffer_size: Size<i32, Buffer>,
    opaque_regions: Vec<Rectangle<i32, Logical>>,
    location: Point<i32, Physical>,
}

impl MemoryRenderBufferRenderElement {
    /// Create a new [`MemoryRenderBufferRenderElement`] for
    /// a [`MemoryRenderBuffer`]
    pub fn from_buffer(
        location: Point<i32, Physical>,
        buffer: &MemoryRenderBuffer,
        buffer_scale: i32,
        buffer_transform: Transform,
        buffer_size: Size<i32, Buffer>,
        opaque_regions: Vec<Rectangle<i32, Logical>>,
    ) -> Self {
        MemoryRenderBufferRenderElement {
            buffer: buffer.clone(),
            buffer_scale,
            buffer_transform,
            buffer_size,
            opaque_regions,
            location,
        }
    }
}

impl<R> RenderElement<R> for MemoryRenderBufferRenderElement
where
    R: Renderer + ImportMem,
    <R as Renderer>::TextureId: 'static,
{
    fn id(&self) -> &super::Id {
        &self.buffer.id
    }

    fn current_commit(&self) -> crate::backend::renderer::utils::CommitCounter {
        self.buffer.current_commit()
    }

    fn geometry(
        &self,
        scale: crate::utils::Scale<f64>,
    ) -> crate::utils::Rectangle<i32, crate::utils::Physical> {
        Rectangle::from_loc_and_size(
            self.location,
            self.buffer_size
                .to_logical(self.buffer_scale, self.buffer_transform)
                .to_physical_precise_round(scale),
        )
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: crate::utils::Scale<f64>,
        damage: &[crate::utils::Rectangle<i32, crate::utils::Physical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error> {
        let mut guard = self.buffer.inner.lock().unwrap();
        let texture = guard.import_texture(renderer, self.buffer_size, log)?;
        frame.render_texture_at(
            texture,
            self.location,
            self.buffer_scale,
            scale,
            self.buffer_transform,
            damage,
            1.0,
        )
    }

    fn damage_since(
        &self,
        scale: crate::utils::Scale<f64>,
        commit: Option<crate::backend::renderer::utils::CommitCounter>,
    ) -> Vec<crate::utils::Rectangle<i32, crate::utils::Physical>> {
        self.buffer
            .damage_since(commit)
            .map(|damage| {
                damage
                    .into_iter()
                    .map(|damage| {
                        damage
                            .to_logical(self.buffer_scale, self.buffer_transform, &self.buffer_size)
                            .to_physical_precise_up(scale)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![Rectangle::from_loc_and_size(
                    Point::default(),
                    RenderElement::<R>::geometry(self, scale).size,
                )]
            })
    }

    fn opaque_regions(
        &self,
        scale: crate::utils::Scale<f64>,
    ) -> Vec<crate::utils::Rectangle<i32, crate::utils::Physical>> {
        self.opaque_regions
            .iter()
            .map(|r| r.to_physical_precise_round(scale))
            .collect::<Vec<_>>()
    }
}
