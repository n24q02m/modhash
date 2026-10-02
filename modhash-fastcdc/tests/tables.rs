//! Bit-for-bit pins for the two constant tables the whole crate stands on.
//!
//! `GEAR` and `MASKS` are the two places a silent one-bit transcription
//! error destroys every downstream property (the design spec calls the mask
//! table the most error-prone constant in the kit). The expected values are
//! written out a second time below — independently of `src/` — so a test
//! failure always means the two copies disagree.
//!
//! `GEAR` provenance: `GEAR[i]` is the first eight bytes, big-endian, of
//! `MD5([i; 64])` — verified offline during implementation for all 256
//! entries against Python `hashlib`, so this pin also stands in for a
//! published-vectors test the paper never provided.

use modhash_fastcdc::{GEAR, MASKS};

/// The 26-slot mask table, re-transcribed from `docs/algorithms/fastcdc.md`.
/// `MASKS[i]` has exactly `i` one-bits; slots 6..=21 are the sixteen usable
/// levels (64 B through 2 MiB).
const EXPECTED_MASKS: [u64; 26] = [
    0x0000_0000_0000_0000,
    0x0000_0000_0000_0000,
    0x0000_0000_0000_0000,
    0x0000_0000_0000_0000,
    0x0000_0000_0000_0000,
    0x0000_0000_0180_4110,
    0x0000_0000_0180_3110,
    0x0000_0000_1803_5100,
    0x0000_0018_0003_5300,
    0x0000_0190_0035_3000,
    0x0000_5900_0353_0000,
    0x0000_d900_0353_0000,
    0x0000_d901_0353_0000,
    0x0000_d903_0353_0000,
    0x0000_d903_1353_0000,
    0x0000_d90f_0353_0000,
    0x0000_d903_0353_7000,
    0x0000_d907_0353_7000,
    0x0000_d907_0753_7000,
    0x0000_d917_0753_7000,
    0x0000_d917_4753_7000,
    0x0000_d917_6753_7000,
    0x0000_d937_6753_7000,
    0x0000_d937_7753_7000,
    0x0000_d937_7757_7000,
    0x0000_db37_7757_7000,
];

/// The 256-entry Gear table, re-transcribed from `docs/algorithms/fastcdc.md`.
#[rustfmt::skip]
const EXPECTED_GEAR: [u64; 256] = [
    0x3b5d3c7d207e37dcu64, 0x784d68ba91123086u64, 0xcd52880f882e7298u64, 0xeacf8e4e19fdcca7u64, 0xc31f385dfbd1632bu64, 0x1d5f27001e25abe6u64, 0x83130bde3c9ad991u64, 0xc4b225676e9b7649u64, 0xaa329b29e08eb499u64, 0xb67fcbd21e577d58u64, 0x0027baaada2acf6bu64, 0xe3ef2d5ac73c2226u64, 0x0890f24d6ed312b7u64, 0xa809e036851d7c7eu64, 0xf0a6fe5e0013d81bu64, 0x1d026304452cec14u64,
    0x03864632648e248fu64, 0xcdaacf3dcd92b9b4u64, 0xf5e012e63c187856u64, 0x8862f9d3821c00b6u64, 0xa82f7338750f6f8au64, 0x1e583dc6c1cb0b6fu64, 0x7a3145b69743a7f1u64, 0xabb20fee404807ebu64, 0xb14b3cfe07b83a5du64, 0xb9dc27898adb9a0fu64, 0x3703f5e91baa62beu64, 0xcf0bb866815f7d98u64, 0x3d9867c41ea9dcd3u64, 0x1be1fa65442bf22cu64, 0x14300da4c55631d9u64, 0xe698e9cbc6545c99u64,
    0x4763107ec64e92a5u64, 0xc65821fc65696a24u64, 0x76196c064822f0b7u64, 0x485be841f3525e01u64, 0xf652bc9c85974ff5u64, 0xcad8352face9e3e9u64, 0x2a6ed1dceb35e98eu64, 0xc6f483badc11680fu64, 0x3cfd8c17e9cf12f1u64, 0x89b83c5e2ea56471u64, 0xae665cfd24e392a9u64, 0xec33c4e504cb8915u64, 0x3fb9b15fc9fe7451u64, 0xd7fd1fd1945f2195u64, 0x31ade0853443efd8u64, 0x255efc9863e1e2d2u64,
    0x10eab6008d5642cfu64, 0x46f04863257ac804u64, 0xa52dc42a789a27d3u64, 0xdaaadf9ce77af565u64, 0x6b479cd53d87febbu64, 0x6309e2d3f93db72fu64, 0xc5738ffbaa1ff9d6u64, 0x6bd57f3f25af7968u64, 0x67605486d90d0a4au64, 0xe14d0b9663bfbdaeu64, 0xb7bbd8d816eb0414u64, 0xdef8a4f16b35a116u64, 0xe7932d85aaaffed6u64, 0x08161cbae90cfd48u64, 0x855507beb294f08bu64, 0x91234ea6ffd399b2u64,
    0xad70cf4b2435f302u64, 0xd289a97565bc2d27u64, 0x8e558437ffca99deu64, 0x96d2704b7115c040u64, 0x0889bbcdfc660e41u64, 0x5e0d4e67dc92128du64, 0x72a9f8917063ed97u64, 0x438b69d409e016e3u64, 0xdf4fed8a5d8a4397u64, 0x00f41dcf41d403f7u64, 0x4814eb038e52603fu64, 0x9dafbacc58e2d651u64, 0xfe2f458e4be170afu64, 0x4457ec414df6a940u64, 0x06e62f1451123314u64, 0xbd1014d173ba92ccu64,
    0xdef318e25ed57760u64, 0x9fea0de9dfca8525u64, 0x459de1e76c20624bu64, 0xaeec189617e2d666u64, 0x126a2c06ab5a83cbu64, 0xb1321532360f6132u64, 0x65421503dbb40123u64, 0x2d67c287ea089ab3u64, 0x6c93bff5a56bd6b6u64, 0x4ffb2036cab6d98du64, 0xce7b785b1be7ad4fu64, 0xedb42ef6189fd163u64, 0xdc905288703988f6u64, 0x365f9c1d2c691884u64, 0xc640583680d99bfeu64, 0x3cd4624c07593ec6u64,
    0x7f1ea8d85d7c5805u64, 0x014842d480b57149u64, 0x0b649bcb5a828688u64, 0xbcd5708ed79b18f0u64, 0xe987c862fbd2f2f0u64, 0x982731671f0cd82cu64, 0xbaf13e8b16d8c063u64, 0x8ea3109cbd951bbau64, 0xd141045bfb385cadu64, 0x2acbc1a0af1f7d30u64, 0xe6444d89df03bfdfu64, 0xa18cc771b8188ff9u64, 0x9834429db01c39bbu64, 0x214add07fe086a1fu64, 0x8f07c19b1f6b3ff9u64, 0x56a297b1bf4ffe55u64,
    0x94d558e493c54fc7u64, 0x40bfc24c764552cbu64, 0x931a706f8a8520cbu64, 0x32229d322935bd52u64, 0x2560d0f5dc4fefafu64, 0x9dbcc48355969bb6u64, 0x0fd81c3985c0b56au64, 0xe03817e1560f2bdau64, 0xc1bb4f81d892b2d5u64, 0xb0c4864f4e28d2d7u64, 0x3ecc49f9d9d6c263u64, 0x51307e99b52ba65eu64, 0x8af2b688da84a752u64, 0xf5d72523b91b20b6u64, 0x6d95ff1ff4634806u64, 0x562f21555458339au64,
    0xc0ce47f889336346u64, 0x487823e5089b40d8u64, 0xe4727c7ebc6d9592u64, 0x5a8f7277e94970bau64, 0xfca2f406b1c8bb50u64, 0x5b1f8a95f1791070u64, 0xd304af9fc9028605u64, 0x5440ab7fc930e748u64, 0x312d25fbca2ab5a1u64, 0x10f4a4b234a4d575u64, 0x90301d55047e7473u64, 0x3b6372886c61591eu64, 0x293402b77c444e06u64, 0x451f34a4d3e97dd7u64, 0x3158d814d81bc57bu64, 0x034942425b9bda69u64,
    0xe2032ff9e532d9bbu64, 0x62ae066b8b2179e5u64, 0x9545e10c2f8d71d8u64, 0x7ff7483eb2d23fc0u64, 0x00945fcebdc98d86u64, 0x8764bbbe99b26ca2u64, 0x1b1ec62284c0bfc3u64, 0x58e0fcc4f0aa362bu64, 0x5f4abefa878d458du64, 0xfd74ac2f9607c519u64, 0xa4e3fb37df8cbfa9u64, 0xbf697e43cac574e5u64, 0x86f14a3f68f4cd53u64, 0x24a23d076f1ce522u64, 0xe725cd8048868cc8u64, 0xbf3c729eb2464362u64,
    0xd8f6cd57b3cc1ed8u64, 0x6329e52425541577u64, 0x62aa688ad5ae1ac0u64, 0x0a242566269bf845u64, 0x168b1a4753aca74bu64, 0xf789afefff2e7e3cu64, 0x6c3362093b6fccdbu64, 0x4ce8f50bd28c09b2u64, 0x006a2db95ae8aa93u64, 0x975b0d623c3d1a8cu64, 0x18605d3935338c5bu64, 0x5bb6f6136cad3c71u64, 0x0f53a20701f8d8a6u64, 0xab8c5ad2e7e93c67u64, 0x40b5ac5127acaa29u64, 0x8c7bf63c2075895fu64,
    0x78bd9f7e014a805cu64, 0xb2c9e9f4f9c8c032u64, 0xefd6049827eb91f3u64, 0x2be459f482c16fbdu64, 0xd92ce0c5745aaa8cu64, 0x0aaa8fb298d965b9u64, 0x2b37f92c6c803b15u64, 0x8c54a5e94e0f0e78u64, 0x95f9b6e90c0a3032u64, 0xe7939faa436c7874u64, 0xd16bfe8f6a8a40c9u64, 0x44982b86263fd2fau64, 0xe285fb39f984e583u64, 0x779a8df72d7619d3u64, 0xf2d79a8de8d5dd1eu64, 0xd1037354d66684e2u64,
    0x004c82a4e668a8e5u64, 0x31d40a7668b044e6u64, 0xd70578538bd02c11u64, 0xdb45431078c5f482u64, 0x977121bb7f6a51adu64, 0x73d5ccbd34eff8ddu64, 0xe437a07d356e17cdu64, 0x47b2782043c95627u64, 0x9fb251413e41d49au64, 0xccd70b60652513d3u64, 0x1c95b31e8a1b49b2u64, 0xcae73dfd1bcb4c1bu64, 0x34d98331b1f5b70fu64, 0x784e39f22338d92fu64, 0x18613d4a064df420u64, 0xf1d8dae25f0bcebeu64,
    0x33f77c15ae855efcu64, 0x3c88b3b912eb109cu64, 0x956a2ec96bafeea5u64, 0x1aa005b5e0ad0e87u64, 0x5500d70527c4bb8eu64, 0xe36c57196421cc44u64, 0x13c4d286cc36ee39u64, 0x5654a23d818b2a81u64, 0x77b1dc13d161abdcu64, 0x734f44de5f8d5eb5u64, 0x60717e174a6c89a2u64, 0xd47d9649266a211eu64, 0x5b13a4322bb69e90u64, 0xf7669609f8b5fc3cu64, 0x21e6ac55bedcdac9u64, 0x9b56b62b61166deau64,
    0xf48f66b939797e9cu64, 0x35f332f9c0e6ae9au64, 0xcc733f6a9a878db0u64, 0x3da161e41cc108c2u64, 0xb7d74ae535914d51u64, 0x4d493b0b11d36469u64, 0xce264d1dfba9741au64, 0xa9d1f2dc7436dc06u64, 0x70738016604c2a27u64, 0x231d36e96e93f3d5u64, 0x7666881197838d19u64, 0x4a2a83090aaad40cu64, 0xf1e761591668b35du64, 0x7363236497f730a7u64, 0x301080e37379dd4du64, 0x502dea2971827042u64,
    0xc2c5eb858f32625fu64, 0x786afb9edfafbdffu64, 0xdaee0d868490b2a4u64, 0x617366b3268609f6u64, 0xae0e35a0fe46173eu64, 0xd1a07de93e824f11u64, 0x079b8b115ea4cca8u64, 0x93a99274558faebbu64, 0xfb1e6e22e08a03b3u64, 0xea635fdba3698dd0u64, 0xcf53659328503a5cu64, 0xcde3b31e6fd5d780u64, 0x8e3e4221d3614413u64, 0xef14d0d86bf1a22cu64, 0xe1d830d3f16c5ddbu64, 0xaabd2b2a451504e1u64,
];

#[test]
fn mask_table_matches_the_spec_verbatim() {
    assert_eq!(MASKS, EXPECTED_MASKS);
}

#[test]
fn mask_table_has_index_equal_popcount() {
    // Structural invariant from the paper's construction: MASKS[i] exists to
    // target a 2^i-byte average, so it carries i one-bits. This catches a
    // class of mistakes literal-pinning alone cannot — e.g. a shifted nibble
    // that keeps the value wrong but plausible-looking.
    for (i, &mask) in MASKS.iter().enumerate() {
        if i >= 5 {
            assert_eq!(mask.count_ones() as usize, i, "MASKS[{i}] bit count");
        } else {
            assert_eq!(mask, 0, "MASKS[{i}] must be padding");
        }
    }
}

#[test]
fn paper_algorithm1_masks_are_the_table_entries() {
    // Algorithm 1 of the 2016 paper uses masks with 15, 13 and 11 one-bits
    // for an 8 KiB target at normalization level 2: the table maps those to
    // indices 15, 13 and 11. The paper's literal hex differs from the table
    // (paper: 0x00003590703530000-style placements; table: spread bits), so
    // this pins the bit counts, which is what the algorithm depends on.
    assert_eq!(MASKS[15].count_ones(), 15); // MaskS role
    assert_eq!(MASKS[13].count_ones(), 13); // MaskA role
    assert_eq!(MASKS[11].count_ones(), 11); // MaskL role
}

#[test]
fn gear_table_matches_the_spec_verbatim() {
    assert_eq!(GEAR, EXPECTED_GEAR);
}

#[test]
fn gear_table_entries_are_all_distinct() {
    // A byte-to-constant collision would make two different bytes
    // indistinguishable in the window. With 256 random-looking u64s the
    // chance of an accidental collision is ~2^-58; an assert is free.
    let mut sorted = GEAR;
    sorted.sort_unstable();
    for w in sorted.windows(2) {
        assert_ne!(w[0], w[1]);
    }
}

#[test]
fn gear_table_corner_entries() {
    // Spot pins at the edges where transcription tools most often slip:
    // first, last and a mid-table entry.
    assert_eq!(GEAR[0], 0x3b5d3c7d207e37dc);
    assert_eq!(GEAR[128], 0xc0ce47f889336346);
    assert_eq!(GEAR[255], 0xaabd2b2a451504e1);
}
