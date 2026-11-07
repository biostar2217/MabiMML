//! 작곡 랭크별 파트 글자 수 제한 (멜로디, 화음1, 화음2). 출처: Mabinogi World Wiki 'Compose'.

pub const UNLIMITED: i64 = 1_000_000_000;

pub const RANK_LIMITS: [(&str, [i64; 3]); 16] = [
    ("연습", [200, 100, 0]),
    ("F", [400, 200, 100]),
    ("E", [500, 200, 100]),
    ("D", [600, 250, 150]),
    ("C", [650, 250, 200]),
    ("B", [700, 300, 200]),
    ("A", [750, 300, 200]),
    ("9", [800, 350, 200]),
    ("8", [850, 400, 200]),
    ("7", [900, 400, 200]),
    ("6", [950, 450, 200]),
    ("5", [1000, 500, 250]),
    ("4", [1050, 550, 300]),
    ("3", [1100, 600, 350]),
    ("2", [1150, 700, 400]),
    ("1", [1200, 800, 500]),
];

/// '노래 파트 제외' 옵션을 켰을 때
pub const RANK_LIMITS_NO_SONG: [(&str, [i64; 3]); 16] = [
    ("연습", [266, 166, 66]),
    ("F", [553, 333, 233]),
    ("E", [666, 366, 266]),
    ("D", [800, 450, 350]),
    ("C", [866, 466, 416]),
    ("B", [933, 533, 433]),
    ("A", [1000, 550, 450]),
    ("9", [1066, 616, 466]),
    ("8", [1133, 683, 483]),
    ("7", [1200, 700, 500]),
    ("6", [1266, 766, 516]),
    ("5", [1333, 833, 583]),
    ("4", [1400, 900, 650]),
    ("3", [1466, 966, 716]),
    ("2", [1533, 1083, 783]),
    ("1", [1600, 1200, 900]),
];

pub fn rank_order() -> Vec<&'static str> {
    RANK_LIMITS.iter().map(|(r, _)| *r).collect()
}

pub fn limits_for(rank: &str, no_song: bool, custom: Option<[i64; 3]>) -> Result<[i64; 3], String> {
    if rank == "무제한" {
        return Ok([UNLIMITED; 3]);
    }
    if let Some(c) = custom {
        return Ok(c);
    }
    let table = if no_song { &RANK_LIMITS_NO_SONG } else { &RANK_LIMITS };
    let find = |r: &str| table.iter().find(|(k, _)| *k == r).map(|(_, v)| *v);
    let mut r = if find(rank).is_some() { rank.to_string() } else { rank.to_uppercase() };
    if r == "N" || r == "NOVICE" || r == "PRACTICE" {
        r = "연습".into();
    }
    find(&r).ok_or_else(|| {
        format!("알 수 없는 랭크: {} (사용 가능: {}, 무제한)", rank, rank_order().join(", "))
    })
}
