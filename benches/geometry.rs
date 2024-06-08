use criterion::{criterion_group, criterion_main, Criterion};
use rand::Rng;
use smithay::utils::{Physical, Rectangle, Size};

fn element_visible_size(
    test_element: Rectangle<i32, Physical>,
    opaque_regions: &Vec<Rectangle<i32, Physical>>,
) {
    let mut workhouse = Vec::with_capacity(2048 * 4);
    workhouse.push(test_element);
    workhouse = Rectangle::subtract_rects_many_in_place(workhouse, opaque_regions.iter().copied());
    workhouse
        .iter()
        .fold(0usize, |acc, item| acc + (item.size.w * item.size.h) as usize);
}

fn criterion_benchmark(c: &mut Criterion) {
    let stage: Size<i32, Physical> = Size::from((800, 600));
    let element_size: Size<i32, Physical> = Size::from((200, 100));
    let max_x = stage.w - element_size.w;
    let max_y = stage.h - element_size.h;

    let mut rand = rand::thread_rng();
    let x = rand.gen_range(0..max_x);
    let y = rand.gen_range(0..max_y);
    let test_element = Rectangle::from_loc_and_size((x, y), element_size);

    let x_min = (test_element.loc.x - element_size.w) + 1;
    let x_max = (test_element.loc.x + element_size.w) - 1;
    let y_min = (test_element.loc.y - element_size.h) + 1;
    let y_max = (test_element.loc.y + element_size.h) - 1;
    // let x_min = 0;
    // let x_max = stage.w - element_size.w;
    // let y_min = 0;
    // let y_max = stage.h - element_size.h;

    let opaque_regions = (0..2048)
        .into_iter()
        .map(|_| {
            let x = rand.gen_range(x_min..=x_max);
            let y = rand.gen_range(y_min..=y_max);
            Rectangle::from_loc_and_size((x, y), element_size)
        })
        .collect::<Vec<_>>();

    c.bench_function("element_visible_size", |b| {
        b.iter(|| {
            element_visible_size(test_element, &opaque_regions);
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
