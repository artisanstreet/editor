const THRESHOLDS: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

pub struct Source<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub pixel_size: f32,
    pub contrast: f32,
    pub brightness: f32,
    pub threshold_bias: f32,
    pub max_resolution: f32,
    pub fade_right: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub rgb: [u8; 3],
    pub opacity: f32,
    pub fade_in_left: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub visible: Vec<bool>,
}

#[must_use]
pub fn grid(box_width: f32, box_height: f32, params: Params) -> (u32, u32) {
    let resolution = params.max_resolution.max(64.0);
    let sample = params.pixel_size.max(0.5);
    let sampled = ((box_width / sample).max(1.0), (box_height / sample).max(1.0));
    let reduction = 1.0_f32
        .min(resolution / sampled.0)
        .min(resolution * 1.5 / sampled.1);
    (
        (sampled.0 * reduction).round().max(1.0) as u32,
        (sampled.1 * reduction).round().max(1.0) as u32,
    )
}

#[must_use]
pub fn dither(source: &Source<'_>, box_width: f32, box_height: f32, params: Params) -> Mask {
    let (width, height) = grid(box_width, box_height, params);
    let scale = (width as f32 / source.width as f32).min(height as f32 / source.height as f32);
    let drawn = (source.width as f32 * scale, source.height as f32 * scale);
    let top = height as f32 - drawn.1;
    let fade_right = params.fade_right.clamp(0.0, 1.0);
    let visible = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let Some([r, g, b, a]) = area(source, (x as f32) / scale, (y as f32 - top) / scale, 1.0 / scale) else {
                return false;
            };
            let luminance = (r * 0.2126 + g * 0.7152 + b * 0.0722) / 255.0;
            let threshold = (f32::from(THRESHOLDS[((y % 4) * 4 + x % 4) as usize]) + 0.5) / 16.0;
            let fade = match fade_right > 0.0 {
                true => 1.0 - ((x as f32 / width as f32 - (1.0 - fade_right)) / fade_right).max(0.0),
                false => 1.0,
            };
            let adjusted = ((luminance * params.brightness - 0.5) * params.contrast + 0.5).clamp(0.0, 1.0) * fade;
            adjusted > threshold + params.threshold_bias && a > 0.0
        })
        .collect();
    Mask {
        width,
        height,
        visible,
    }
}

#[must_use]
pub fn paint(mask: &Mask, paint: Paint) -> Vec<u8> {
    let width = mask.width as f32;
    mask.visible
        .iter()
        .enumerate()
        .flat_map(|(index, visible)| {
            let x = (index as u32 % mask.width) as f32;
            let reveal = match paint.fade_in_left > 0.0 {
                true => ((x + 0.5) / width / paint.fade_in_left).min(1.0),
                false => 1.0,
            };
            let alpha = match visible {
                true => (255.0 * paint.opacity * reveal).round() as u8,
                false => 0,
            };
            [paint.rgb[0], paint.rgb[1], paint.rgb[2], alpha]
        })
        .collect()
}

fn area(source: &Source<'_>, left: f32, top: f32, span: f32) -> Option<[f32; 4]> {
    let right = left + span;
    let bottom = top + span;
    if right <= 0.0 || bottom <= 0.0 || left >= source.width as f32 || top >= source.height as f32 {
        return None;
    }
    let columns = (left.max(0.0).floor() as u32)..(right.min(source.width as f32).ceil() as u32);
    let rows = (top.max(0.0).floor() as u32)..(bottom.min(source.height as f32).ceil() as u32);
    let mut total = [0.0_f32; 4];
    let mut weight = 0.0_f32;
    for y in rows {
        let cover_y = (bottom.min(y as f32 + 1.0) - top.max(y as f32)).max(0.0);
        for x in columns.clone() {
            let cover = cover_y * (right.min(x as f32 + 1.0) - left.max(x as f32)).max(0.0);
            let at = ((y * source.width + x) * 4) as usize;
            for (channel, sum) in total.iter_mut().enumerate() {
                *sum += f32::from(source.rgba[at + channel]) * cover;
            }
            weight += cover;
        }
    }
    (weight > 0.0).then(|| total.map(|sum| sum / weight))
}
