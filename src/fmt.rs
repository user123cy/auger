pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn ms(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.0}", v)
    } else {
        format!("{:.1}", v)
    }
}

pub fn dec1(v: f64) -> String {
    format!("{:.1}", v)
}

pub fn whole(v: f64) -> String {
    format!("{:.0}", v)
}