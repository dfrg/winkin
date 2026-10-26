# unicode_gen

Generates cleft's packed Unicode property and case tables
(`crates/cleft/src/props/tables.rs`, `crates/cleft/src/case/tables.rs`) and
its word dictionaries (`crates/cleft/src/dictionary/data/*.dafsa`). The
generated files are committed; run the generator only to change them.

```sh
cargo run --release -p unicode_gen -- fetch         # download the inputs
cargo run --release -p unicode_gen                  # property and case tables, Unicode 18.0.0
cargo run --release -p unicode_gen -- 17.0.0        # the same from another pinned version
cargo run --release -p unicode_gen -- dictionaries  # word dictionaries
cargo fmt
```

Running it again on the same inputs reproduces the committed files byte for
byte.

## Inputs

The generator reads Unicode Character Database files and ICU's word lists
from `support/unicode_gen/data/`. They are about 24 MB and are not in the
repository. [`manifest.txt`](manifest.txt) lists each file with its SHA-256
and the URL it comes from; the generator refuses a file that is missing,
unlisted or changed, and stops with an error that names the file.

`cargo run -p unicode_gen -- fetch` downloads every listed file into
`data/` with `curl` and checks its hash. It skips files already present with
the right hash, and removes and reports a download whose hash differs. To
fetch by hand, save each URL below to `data/<file>` and check its SHA-256.

The same files serve cleft's conformance and sweep tests (the UAX #29 and
UAX #14 test files, and comparisons with ICU4X across Unicode versions) and
winkin's grapheme cluster test. Without them those tests print that the
file is absent and pass without checking; fetch the files to run them in
full.

| File | Source | SHA-256 |
|---|---|---|
| `15.1.0/LineBreak.txt` | <https://www.unicode.org/Public/15.1.0/ucd/LineBreak.txt> | `a55cdc118493491e0a9c885b48eda953a6258a1e218f6050a11891231c79683f` |
| `15.1.0/LineBreakTest_15.1.txt` | <https://raw.githubusercontent.com/unicode-org/icu4x/icu%402.3.0/components/segmenter/tests/testdata/LineBreakTest_15.1.txt> | `ad9df68e0fb5bbcf89194da6029612d41f3a028f331df4a03d904513fd11c3ef` |
| `17.0.0/BidiBrackets.txt` | <https://www.unicode.org/Public/17.0.0/ucd/BidiBrackets.txt> | `dadbaf38a0d0246e5b805bf8725cb81b7c621f93d030595635f5ba2c2f179428` |
| `17.0.0/DerivedBidiClass.txt` | <https://www.unicode.org/Public/17.0.0/ucd/extracted/DerivedBidiClass.txt> | `4867b4b7f0731ed1bfcd34cc6251211ff1542541fce0734b6fbda139ee80b3a4` |
| `17.0.0/DerivedCoreProperties.txt` | <https://www.unicode.org/Public/17.0.0/ucd/DerivedCoreProperties.txt> | `24c7fed1195c482faaefd5c1e7eb821c5ee1fb6de07ecdbaa64b56a99da22c08` |
| `17.0.0/DerivedGeneralCategory.txt` | <https://www.unicode.org/Public/17.0.0/ucd/extracted/DerivedGeneralCategory.txt> | `d62e5bab70ca74f099343f71224fa051cb1fdd61a1ab45c0488c44cfc0b6102e` |
| `17.0.0/DerivedJoiningType.txt` | <https://www.unicode.org/Public/17.0.0/ucd/extracted/DerivedJoiningType.txt> | `f39ebe974825d6736aee15582250307aa532b2cfab3caf3f86bd23fddc9c5c4d` |
| `17.0.0/EastAsianWidth.txt` | <https://www.unicode.org/Public/17.0.0/ucd/EastAsianWidth.txt> | `ea7ce50f3444a050333448dffef1cadd9325af55cbb764b4a2280faf52170a33` |
| `17.0.0/GraphemeBreakProperty.txt` | <https://www.unicode.org/Public/17.0.0/ucd/auxiliary/GraphemeBreakProperty.txt> | `d6b51d1d2ae5c33b451b7ed994b48f1f4dc62b2272a5831e7fd418514a6bae89` |
| `17.0.0/GraphemeBreakTest.txt` | <https://www.unicode.org/Public/17.0.0/ucd/auxiliary/GraphemeBreakTest.txt> | `e2d134d2c52919bace503ebb6a551c1855fe1a1faec18478c78fff254a1793ec` |
| `17.0.0/LineBreak.txt` | <https://www.unicode.org/Public/17.0.0/ucd/LineBreak.txt> | `e6a18fa91f8f6a6f8e534b1d3f128c21ada45bfe152eb6b1bcc5e15fd8ac92e6` |
| `17.0.0/PropList.txt` | <https://www.unicode.org/Public/17.0.0/ucd/PropList.txt> | `130dcddcaadaf071008bdfce1e7743e04fdfbc910886f017d9f9ac931d8c64dd` |
| `17.0.0/PropertyValueAliases.txt` | <https://www.unicode.org/Public/17.0.0/ucd/PropertyValueAliases.txt> | `64e9a5f76f7a1e8b5a47d6a1f9a26522a251208f5276bdfa1559dac7cf2e827a` |
| `17.0.0/ScriptExtensions.txt` | <https://www.unicode.org/Public/17.0.0/ucd/ScriptExtensions.txt> | `ec2107e58825a1586acee8e0911ce18260394ac8b87e535ca325f1ccbeb06bc6` |
| `17.0.0/Scripts.txt` | <https://www.unicode.org/Public/17.0.0/ucd/Scripts.txt> | `9f5e50d3abaee7d6ce09480f325c706f485ae3240912527e651954d2d6b035bf` |
| `17.0.0/SpecialCasing.txt` | <https://www.unicode.org/Public/17.0.0/ucd/SpecialCasing.txt> | `efc25faf19de21b92c1194c111c932e03d2a5eaf18194e33f1156e96de4c9588` |
| `17.0.0/UnicodeData.txt` | <https://www.unicode.org/Public/17.0.0/ucd/UnicodeData.txt> | `2e1efc1dcb59c575eedf5ccae60f95229f706ee6d031835247d843c11d96470c` |
| `17.0.0/VerticalOrientation.txt` | <https://www.unicode.org/Public/17.0.0/ucd/VerticalOrientation.txt> | `dcef09c3fb24d356b042569c328ec341efc5b53447700d799f2fb4834c3cd3cd` |
| `17.0.0/WordBreakProperty.txt` | <https://www.unicode.org/Public/17.0.0/ucd/auxiliary/WordBreakProperty.txt> | `72274cac1e6b919507db35655c3e175aa27274668a1ece95c28d2069f2ad9852` |
| `17.0.0/WordBreakTest.txt` | <https://www.unicode.org/Public/17.0.0/ucd/auxiliary/WordBreakTest.txt> | `1de23a75f37904abc7d206239ee8d34f8fdf0fb4ab32a7174dfbabbde25419b2` |
| `17.0.0/emoji-data.txt` | <https://www.unicode.org/Public/17.0.0/ucd/emoji/emoji-data.txt> | `2cb2bb9455cda83e8481541ecf5b6dfda66a3bb89efa3fa7c5297eccf607b72b` |
| `18.0.0/BidiBrackets.txt` | <https://www.unicode.org/Public/18.0.0/ucd/BidiBrackets.txt> | `4b3b62e4a14b84ee752808c810c602534921c09a4a1bf78cfbee566d66c125b3` |
| `18.0.0/DerivedBidiClass.txt` | <https://www.unicode.org/Public/18.0.0/ucd/extracted/DerivedBidiClass.txt> | `d9e23222522551348ea1ccfbb4f62efbf98982afb95840f8959c08ed992c5607` |
| `18.0.0/DerivedCoreProperties.txt` | <https://www.unicode.org/Public/18.0.0/ucd/DerivedCoreProperties.txt> | `09c928886a178fcafd93c29e4bd59073a058e5a100b716d425cb563ab50f68c9` |
| `18.0.0/DerivedGeneralCategory.txt` | <https://www.unicode.org/Public/18.0.0/ucd/extracted/DerivedGeneralCategory.txt> | `d6b151d2d40ee9b1876d26f417980f45ffae47b6055ccf7203cb31f07a030f94` |
| `18.0.0/DerivedJoiningType.txt` | <https://www.unicode.org/Public/18.0.0/ucd/extracted/DerivedJoiningType.txt> | `e2408ff2c92b175b0f7bf62c989bbb54c7b077528fe31f8d69b96fa09e7d61ed` |
| `18.0.0/EastAsianWidth.txt` | <https://www.unicode.org/Public/18.0.0/ucd/EastAsianWidth.txt> | `a0cf29eacd00cfcaec4381c6b7c281685f18dbb4e7ff82b4076ccb342ca839aa` |
| `18.0.0/GraphemeBreakProperty.txt` | <https://www.unicode.org/Public/18.0.0/ucd/auxiliary/GraphemeBreakProperty.txt> | `0839dcb79e4ac639ecd538b1abf7c9d22e3f9dd265b7e182d33627aa4d75b45a` |
| `18.0.0/GraphemeBreakTest.txt` | <https://www.unicode.org/Public/18.0.0/ucd/auxiliary/GraphemeBreakTest.txt> | `b0cf047ee94485bbdc846de2b902f5f8a815f6b674f9d04223cddadd91c9df31` |
| `18.0.0/LineBreak.txt` | <https://www.unicode.org/Public/18.0.0/ucd/LineBreak.txt> | `ae8cf1970c73f3f1a12d77852df96c4b2b1723ca8b388e36bb5739250c6849de` |
| `18.0.0/LineBreakTest.txt` | <https://www.unicode.org/Public/18.0.0/ucd/auxiliary/LineBreakTest.txt> | `fd9ff9eca411080085b41471bba79ae19d9db2e16924f9770b8509f1affd3f95` |
| `18.0.0/PropList.txt` | <https://www.unicode.org/Public/18.0.0/ucd/PropList.txt> | `f438f532e8737bb8a2702126cdf9c4af5e357c58c7acf9d9eb2fc7c1a1d955d6` |
| `18.0.0/PropertyValueAliases.txt` | <https://www.unicode.org/Public/18.0.0/ucd/PropertyValueAliases.txt> | `06c4c8eaf7b0bf34abe73b113da1215bd784ac254d4c223600b90267caa4bbbd` |
| `18.0.0/ScriptExtensions.txt` | <https://www.unicode.org/Public/18.0.0/ucd/ScriptExtensions.txt> | `5c9d34a922f687726f2a8bcf57d49f905987e51f1b21b58c95a00fbe255cec23` |
| `18.0.0/Scripts.txt` | <https://www.unicode.org/Public/18.0.0/ucd/Scripts.txt> | `0071fd81b6aeae25f6e8bce8efec3066a6476a91b49bdb2f52dc76e817862a6a` |
| `18.0.0/SpecialCasing.txt` | <https://www.unicode.org/Public/18.0.0/ucd/SpecialCasing.txt> | `8538dea57c184f1ef3783885ea79677b10f6efa06423717157e63712f14d1ad2` |
| `18.0.0/UnicodeData.txt` | <https://www.unicode.org/Public/18.0.0/ucd/UnicodeData.txt> | `0736451de439ae7baf1425136617da495e09ee5afbe6e394374db7009ea08950` |
| `18.0.0/VerticalOrientation.txt` | <https://www.unicode.org/Public/18.0.0/ucd/VerticalOrientation.txt> | `0803e09669d7aa7137de678e55b848bd418c6632694fa1b05f0b40941103c748` |
| `18.0.0/WordBreakProperty.txt` | <https://www.unicode.org/Public/18.0.0/ucd/auxiliary/WordBreakProperty.txt> | `8dbfa17063e11084201f33c3e76d485d3b9166930c71db8e39ed1c9234171aec` |
| `18.0.0/WordBreakTest.txt` | <https://www.unicode.org/Public/18.0.0/ucd/auxiliary/WordBreakTest.txt> | `3dd70c071781276067c680d87303f60434adce7b067bd063194af374edad86a5` |
| `18.0.0/emoji-data.txt` | <https://www.unicode.org/Public/18.0.0/ucd/emoji/emoji-data.txt> | `80d00f8e616a0ef27fd6b8de3b758c06383b5d917e2977709578e68baf733bf1` |
| `icu-78.1rc/burmesedict.txt` | <https://raw.githubusercontent.com/unicode-org/icu/release-78.1rc/icu4c/source/data/brkitr/dictionaries/burmesedict.txt> | `61d8abc3d9102b2f9bf0c9f44db0d7ab89b18172d8cd26832e4c83174bd8673b` |
| `icu-78.1rc/cjdict.txt` | <https://raw.githubusercontent.com/unicode-org/icu/release-78.1rc/icu4c/source/data/brkitr/dictionaries/cjdict.txt> | `e73fd72048981d0cc13e9dc436a7eaba07ffb6eff58c8a59dc75c1df746663a0` |
| `icu-78.1rc/khmerdict.txt` | <https://raw.githubusercontent.com/unicode-org/icu/release-78.1rc/icu4c/source/data/brkitr/dictionaries/khmerdict.txt> | `87bee2d17cd5148aa36957eb05409eefc124de8ad519b81b789298ef3e60b5d9` |
| `icu-78.1rc/laodict.txt` | <https://raw.githubusercontent.com/unicode-org/icu/release-78.1rc/icu4c/source/data/brkitr/dictionaries/laodict.txt> | `3c876934a3fa81031d2333525eafaca6a7c9f842e3b98f18c38880420afb5d36` |
| `icu-78.1rc/thaidict.txt` | <https://raw.githubusercontent.com/unicode-org/icu/release-78.1rc/icu4c/source/data/brkitr/dictionaries/thaidict.txt> | `3166abde40c0f44ab91c28f5ce96d7d1472cb7882e1c0bda0a72f8f69dba4274` |

The UCD files are © Unicode, Inc., under the Unicode License v3
(`crates/cleft/LICENSE-UNICODE`). ICU's word lists are under ICU's
license, which `crates/cleft/THIRD-PARTY` quotes.
