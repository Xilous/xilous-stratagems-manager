#[derive(Clone, Copy)]
struct ColorProfile {
    chroma: [f32; 3],
    luma_low: f32,
    luma_full: f32,
    distance_full: f32,
    distance_zero: f32,
}

impl ColorProfile {
    const fn new(
        rgb: [u8; 3],
        luma_low: f32,
        luma_full: f32,
        distance_full: f32,
        distance_zero: f32,
    ) -> Self {
        let sum = rgb[0] as f32 + rgb[1] as f32 + rgb[2] as f32;
        Self {
            chroma: [
                rgb[0] as f32 / sum,
                rgb[1] as f32 / sum,
                rgb[2] as f32 / sum,
            ],
            luma_low,
            luma_full,
            distance_full,
            distance_zero,
        }
    }
}

#[derive(Clone, Copy)]
struct ColorSample {
    chroma: [f32; 3],
    luma: f32,
}

impl ColorSample {
    #[inline]
    fn from_rgb(r: u8, g: u8, b: u8) -> Option<Self> {
        let sum = r as f32 + g as f32 + b as f32;
        if sum <= 1.0 {
            return None;
        }
        Some(Self {
            chroma: [r as f32 / sum, g as f32 / sum, b as f32 / sum],
            luma: luma601_u8(r, g, b) as f32,
        })
    }
}

// Reference colors and soft ranges measured from representative non-empty slot crops.
const WHITE: ColorProfile = ColorProfile::new([255, 255, 237], 100.0, 180.0, 0.006, 0.035);
const OFFENSIVE_RED: ColorProfile = ColorProfile::new([201, 90, 76], 55.0, 115.0, 0.030, 0.130);
const DEFENSIVE_GREEN: ColorProfile = ColorProfile::new([103, 148, 82], 67.0, 119.0, 0.023, 0.075);
const SUPPLY_BLUE: ColorProfile = ColorProfile::new([77, 177, 206], 60.0, 140.0, 0.020, 0.090);
const BOOSTER_YELLOW_RGB: [u8; 3] = [255, 222, 38];
const BOOSTER_YELLOW_MAX_DISTANCE: u32 = 32;
const BOOSTER_YELLOW: ColorProfile =
    ColorProfile::new(BOOSTER_YELLOW_RGB, 75.0, 160.0, 0.025, 0.100);
const REAL_ICON_COLORS: [ColorProfile; 5] = [
    WHITE,
    OFFENSIVE_RED,
    DEFENSIVE_GREEN,
    SUPPLY_BLUE,
    BOOSTER_YELLOW,
];

#[inline]
pub fn luma601_u8(r: u8, g: u8, b: u8) -> u8 {
    ((77u16 * r as u16 + 150u16 * g as u16 + 29u16 * b as u16 + 128) >> 8) as u8
}

pub fn icon_likeness(r: u8, g: u8, b: u8) -> f32 {
    let Some(sample) = ColorSample::from_rgb(r, g, b) else {
        return 0.0;
    };
    REAL_ICON_COLORS
        .iter()
        .map(|profile| color_likeness(sample, *profile))
        .fold(0.0, f32::max)
}

pub fn is_booster_yellow(r: u8, g: u8, b: u8) -> bool {
    let [rr, rg, rb] = BOOSTER_YELLOW_RGB;
    let [dr, dg, db] = [r.abs_diff(rr), g.abs_diff(rg), b.abs_diff(rb)].map(u32::from);
    dr * dr + dg * dg + db * db <= BOOSTER_YELLOW_MAX_DISTANCE * BOOSTER_YELLOW_MAX_DISTANCE
}

fn color_likeness(sample: ColorSample, profile: ColorProfile) -> f32 {
    let distance = ((sample.chroma[0] - profile.chroma[0]).powi(2)
        + (sample.chroma[1] - profile.chroma[1]).powi(2)
        + (sample.chroma[2] - profile.chroma[2]).powi(2))
    .sqrt();
    let brightness = 0.35 + 0.65 * smoothstep(profile.luma_low, profile.luma_full, sample.luma);
    let chromaticity = 1.0 - smoothstep(profile.distance_full, profile.distance_zero, distance);

    brightness * chromaticity
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
