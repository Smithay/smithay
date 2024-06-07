use std::marker::PhantomData;

use crate::{
    backend::renderer::{gles::GlesRenderer, ImportDma, ImportMem, Renderer, Texture},
    utils::{Buffer, Physical, Point, Rectangle, Scale},
};

use super::{CommitCounter, Element, Id, RenderElement, Wrap};

render_elements! {
    ImportMemTest<R> where R: ImportMem;
    Memory=ImportMemRenderElement,
}

render_elements! {
    ImportMemTest2<=GlesRenderer>;
    Memory=ImportMemRenderElement,
}

render_elements! {
    ImportMemTest3<'a, R, T> where R: ImportMem, T: Texture;
    Memory=ImportMemRenderElement,
    Custom=&'a T,
}

render_elements! {
    ImportMemTest4<R> where R: ImportMem + ImportDma;
    Memory=ImportMemRenderElement,
}

render_elements! {
    ImportMemTest5<'a, R, T> where R: ImportMem + ImportDma, T: Texture;
    Memory=ImportMemRenderElement,
    Custom=&'a T,
}

render_elements! {
    TextureIdTest<R> where R: ImportMem, <R as Renderer>::TextureId: Clone;
    Memory=ImportMemRenderElement,
}

render_elements! {
    TextureIdTest1<R> where R: ImportMem, <R as Renderer>::TextureId: 'static;
    Memory=ImportMemRenderElement,
}

render_elements! {
    TextureIdTest2<'a, R, C> where R: ImportMem, <R as Renderer>::TextureId: 'a;
    Memory=ImportMemRenderElement,
    Custom=&'a C,
}

render_elements! {
    TextureIdTest3<'a, R, C> where R: ImportMem, <R as Renderer>::TextureId: Clone + 'a;
    Memory=ImportMemRenderElement,
    Custom=&'a C,
}

render_elements! {
    Test<='a, GlesRenderer>;
    Surface=TestRenderElement<'a, GlesRenderer>
}

render_elements! {
    Test2<=GlesRenderer>;
    Surface=TestRenderElement2<GlesRenderer>
}

render_elements! {
    Test3<='a, GlesRenderer, C>;
    Surface=TestRenderElement<'a, GlesRenderer>,
    Custom=&'a C,
}

render_elements! {
    Test4<=GlesRenderer, C>;
    Surface=TestRenderElement2<GlesRenderer>,
    Custom=C
}

render_elements! {
    TestG<'a, R>;
    Surface=TestRenderElement<'a, R>
}

render_elements! {
    TestG2<R>;
    Surface=TestRenderElement2<R>
}

render_elements! {
    TestG3<'a, R, C>;
    Surface=TestRenderElement<'a, R>,
    Custom=&'a C,
}

render_elements! {
    TestG4<R, C>;
    Surface=TestRenderElement2<R>,
    Custom=Wrap<C>
}

render_elements! {
    TestG5;
    What=Empty,
}

render_elements! {
    TestG6<'a, R, C>;
    Surface=TestRenderElement<'a, R>,
    Custom=&'a C,
    Custom2=Wrap<C>,
}

render_elements! {
    TestG7<'a, R, C1, C2>;
    Surface=TestRenderElement<'a, R>,
    Custom=Wrap<C1>,
    Custom2=Wrap<C2>,
}

struct ImportMemRenderElement {}

impl Element for ImportMemRenderElement {
    fn id(&self) -> &Id {
        todo!()
    }

    fn current_commit(&self) -> CommitCounter {
        todo!()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        todo!()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        todo!()
    }
}

impl<R> RenderElement<R> for ImportMemRenderElement
where
    R: Renderer + ImportMem,
{
    fn draw(
        &self,
        _frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        todo!()
    }
}

impl Element for Empty {
    fn id(&self) -> &Id {
        todo!()
    }

    fn current_commit(&self) -> CommitCounter {
        todo!()
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        todo!()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        todo!()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        todo!()
    }
}

impl<R> RenderElement<R> for Empty
where
    R: Renderer,
{
    fn draw(
        &self,
        _frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        todo!()
    }
}

struct Empty;

struct TestRenderElement2<R> {
    _phantom: PhantomData<R>,
}

impl<R> Element for TestRenderElement2<R>
where
    R: Renderer,
{
    fn id(&self) -> &Id {
        todo!()
    }

    fn current_commit(&self) -> CommitCounter {
        todo!()
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        todo!()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        todo!()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        todo!()
    }
}

impl<R> RenderElement<R> for TestRenderElement2<R>
where
    R: Renderer,
{
    fn draw(
        &self,
        _frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        todo!()
    }
}

struct TestRenderElement<'a, R> {
    _test: &'a usize,
    _phantom: PhantomData<R>,
}

impl<'a, R> Element for TestRenderElement<'a, R>
where
    R: Renderer,
{
    fn id(&self) -> &Id {
        todo!()
    }

    fn current_commit(&self) -> CommitCounter {
        todo!()
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        todo!()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        todo!()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        todo!()
    }
}

impl<'a, R> RenderElement<R> for TestRenderElement<'a, R>
where
    R: Renderer,
{
    fn draw(
        &self,
        _frame: &mut <R as Renderer>::Frame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <R as Renderer>::Error> {
        todo!()
    }
}
