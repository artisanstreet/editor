use super::*;
use std::cell::Cell;

fn strip(triangles: usize) -> Mesh {
    let mut positions = Vec::with_capacity(triangles * 3);
    #[expect(clippy::cast_precision_loss, reason = "the strip is small")]
    for index in 0..triangles {
        let left = index as f32 / triangles as f32 - 0.5;
        positions.extend([
            Vec3::new(left, -0.5, 0.0),
            Vec3::new(left + 0.01, -0.5, 0.0),
            Vec3::new(left, 0.5, 0.0),
        ]);
    }
    let normals = vec![Vec3::Z; positions.len()];
    Mesh {
        positions,
        normals,
        colors: None,
    }
}

fn look() -> Look {
    Look {
        base: Vec3::ONE,
        edges: None,
    }
}

#[test]
fn a_cancelled_render_stops_at_the_next_check_and_yields_no_frame() {
    let mesh = strip(1000);
    let checks = Cell::new(0_usize);
    let rendered = render(&mesh, &Camera::default(), &look(), 64, 64, 2, &|| {
        checks.set(checks.get() + 1);
        checks.get() >= 5
    });
    assert_eq!(rendered.err(), Some(Cancelled));
    assert_eq!(checks.get(), 5, "no work continues after cancellation");
}
