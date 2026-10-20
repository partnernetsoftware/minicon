//! Native return/remaining-record oracles, independent of the SDK client.
pub fn kinds(system: bool) -> Vec<String> {
    let mut kinds = Vec::new();
    for api in ["file", "wide", "ansi"] {
        for case in [
            "ascii",
            "capacity",
            "mixed",
            "bmp",
            "tail",
            "eof",
            "middle-eof",
            "pending",
            "pair",
            "split",
        ] {
            // Installed build 20348's UTF-8 byte input fails the independent
            // BMP diagnostic even after CP=65001 readback. Keep its failed
            // receipts separate; only wide Unicode and narrow ASCII compare.
            if system && api != "wide" && matches!(case, "bmp" | "tail" | "pair" | "split") {
                continue;
            }
            kinds.push(format!("{api}-{case}"));
        }
    }
    kinds
}
pub fn expected(kind: &str) -> String {
    let (api, case) = kind.split_once('-').unwrap();
    let wide = api == "wide";
    let row =
        |repeat: u32, unit: u32| vec![repeat, unit, if unit < 128 { unit } else { 0 }, 30, 32];
    let (first, second, peek): (Vec<u16>, Vec<u16>, Vec<Vec<u32>>) = match case {
        "ascii" | "pending" => (vec![65, 13, 66], vec![], vec![]),
        "capacity" => (vec![65, 65], vec![65, 66], vec![row(1, 65), row(1, 66)]),
        "mixed" => (vec![65, 66], vec![66], vec![row(1, 66)]),
        "bmp" if wide => (vec![0x4e2d, 233], vec![90], vec![row(1, 90)]),
        "bmp" => (vec![228, 184, 173, 195], vec![169, 90], vec![row(1, 90)]),
        "tail" if wide => (vec![0x4e2d], vec![88], vec![row(1, 88)]),
        "tail" => (vec![228], vec![184, 173, 88], vec![row(1, 88)]),
        "eof" => (
            if api == "file" { vec![] } else { vec![26, 65] },
            vec![66],
            vec![row(1, 66)],
        ),
        "middle-eof" => (vec![65, 26, 66], vec![], vec![]),
        "pair" if wide => (
            vec![0x4e2d, 0xd83d],
            vec![0xde00, 90],
            vec![row(1, 0xde00), row(1, 90)],
        ),
        "pair" => (
            vec![228, 184, 173, 240, 159],
            vec![152, 128, 90],
            vec![row(1, 90)],
        ),
        "split" if wide => (vec![65, 0xd83d], vec![0xde00, 90], vec![]),
        "split" => (vec![65], vec![240, 159, 152, 128, 90], vec![row(1, 0xd83d)]),
        _ => panic!("unknown raw batch case {kind}"),
    };
    format!(
        "first={first:?};second={second:?};queued={};peek={peek:?}",
        peek.len()
    )
}
