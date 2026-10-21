// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Exercise decoding, crops, raw layout and exit status through the shipped CLI.
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Fixture {
    fn compare(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_testkit-ssim"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn png_raw_crop_and_refusals_use_the_cli_contract() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("testkit-ssim-cli-{}", std::process::id())));
    fs::create_dir(&fixture.0).unwrap();
    let a = image::RgbaImage::from_fn(16, 16, |x, y| {
        image::Rgba([((x + y) * 7) as u8, 80, 170, 255])
    });
    let mut b = a.clone();
    for y in 8..16 {
        for x in 8..16 {
            b.put_pixel(x, y, image::Rgba([255, 255, 255, 0]));
        }
    }
    a.save(fixture.0.join("a.png")).unwrap();
    b.save(fixture.0.join("b.png")).unwrap();
    image::RgbaImage::new(17, 16)
        .save(fixture.0.join("wrong.png"))
        .unwrap();
    fs::write(fixture.0.join("a.rgba"), a.as_raw()).unwrap();
    fs::write(fixture.0.join("b.rgba"), b.as_raw()).unwrap();
    let same = fixture.compare(&["a.png", "a.png"]);
    assert!(same.status.success(), "{same:?}");
    assert_eq!(
        String::from_utf8(same.stdout).unwrap(),
        "TESTKIT_SSIM ssim=1.000000000000 threshold=0.99 pass=true\n"
    );
    let changed = fixture.compare(&["a.png", "b.png"]);
    assert_eq!(changed.status.code(), Some(1), "{changed:?}");
    let raw = fixture.compare(&["a.rgba", "b.rgba", "--size", "16x16"]);
    assert_eq!(raw.status.code(), Some(1), "{raw:?}");
    assert_eq!(raw.stdout, changed.stdout, "PNG and raw decoding agree");
    let cropped = fixture.compare(&["a.png", "b.png", "--crop", "0,0,8x8", "--threshold", "1"]);
    assert!(cropped.status.success(), "{cropped:?}");
    for args in [
        vec!["a.png", "wrong.png"],
        vec!["a.png", "wrong.png", "--crop", "0,0,8x8"],
        vec!["a.png", "b.png", "--threshold", "NaN"],
        vec!["a.png", "b.png", "--crop", "4294967295,0,8x8"],
        vec!["a.rgba", "b.rgba", "--size", "4294967295x4294967295"],
    ] {
        let reply = fixture.compare(&args);
        assert_eq!(reply.status.code(), Some(2), "{args:?}: {reply:?}");
    }
    let mut excess = a.into_raw();
    excess.push(0);
    fs::write(fixture.0.join("a.rgba"), excess).unwrap();
    assert_eq!(
        fixture
            .compare(&["a.rgba", "b.rgba", "--size", "16x16"])
            .status
            .code(),
        Some(2)
    );
}
