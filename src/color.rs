pub fn heat(f: f64) -> (u8, u8, u8) {
    let f = f.clamp(0.0, 1.0);
    if f < 0.5 {
        let t = f * 2.0;
        ((50.0 + 190.0 * t) as u8, 200, (120.0 - 60.0 * t) as u8)
    } else {
        let t = (f - 0.5) * 2.0;
        ((240.0 - 10.0 * t) as u8, (200.0 - 130.0 * t) as u8, 60)
    }
}
