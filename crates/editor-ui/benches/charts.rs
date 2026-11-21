use aqua_core::{GpuiHsla, Rect};
use divan::Bencher;
use editor_ui::rich::charts::scene::{Clip, Scene, Shape};

const SERIES: usize = 6;
const POINTS: usize = 2_000;

fn shapes() -> Vec<Shape> {
    let color = GpuiHsla {
        h: 0.6,
        s: 0.5,
        l: 0.5,
        a: 1.0,
    };
    (0..SERIES)
        .flat_map(|series| {
            let points: Vec<(f32, f32)> = (0..POINTS)
                .map(|point| {
                    let x = f32::from(u16::try_from(point).expect("bench: point fits"))
                        / f32::from(u16::try_from(POINTS).expect("bench: count fits"));
                    let phase = f32::from(u8::try_from(series).expect("bench: series fits"));
                    (x, 0.4f32.mul_add(x.mul_add(12.0, phase).sin(), 0.5))
                })
                .collect();
            let mut area = points.clone();
            area.push((1.0, 1.0));
            area.push((0.0, 1.0));
            [
                Shape::Line {
                    points,
                    width: 1.5,
                    color,
                    dash: false,
                },
                Shape::Area {
                    points: area,
                    color,
                },
            ]
        })
        .collect()
}

fn main() {
    divan::main();
}

fn moving(frame: &mut f32) -> Rect {
    *frame += 1.0;
    Rect {
        x: 40.0,
        y: 300.0 - *frame % 200.0,
        w: 640.0,
        h: 240.0,
    }
}

#[divan::bench(sample_count = 50)]
fn first_paint(bencher: Bencher<'_, '_>) {
    let shapes: std::rc::Rc<[Shape]> = shapes().into();
    let mut frame = 0.0;
    bencher.bench_local(|| {
        Scene::of_shapes(std::rc::Rc::clone(&shapes), Clip::Bounds).paint(moving(&mut frame))
    });
}

#[divan::bench(sample_count = 50)]
fn repaint_while_scrolling(bencher: Bencher<'_, '_>) {
    let scene = Scene::of_shapes(shapes(), Clip::Bounds);
    let mut frame = 0.0;
    bencher.bench_local(|| scene.paint(moving(&mut frame)));
}
