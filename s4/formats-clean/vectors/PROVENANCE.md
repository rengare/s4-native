# Provenance of the vectors

Written by the specification side of the process; the implementation side only reads the results.

- **Cipher vectors** (`cipher.tsv`, `data/long-*.keystream`): outputs of the reference implementation for
  synthetic keys and synthetic inputs (zeros, pseudo-random bytes).
- **Codec vectors** (`compress.tsv`, `data/*.stream`, `data/*.plain`): the compressed streams were
  produced by an encoder written by the specification side (the original program contains only a decoder);
  each stream was decoded again by the reference implementation and had to give back exactly the plain data
  it was made from. The plain data is synthetic (pseudo-text, random bytes, byte-value sweeps). The error
  vectors come from the reference implementation's behaviour on deliberately broken inputs.
- **Reference implementation:** checked against all of the original program's real data files (every
  compressed archive entry and every compressed map chunk decodes to its recorded size; encrypted headers
  decode to valid values). No file of the original program or its data is part of this repository.
- **Batch 2 vectors** (`header.tsv`, `texts*.tsv`, `archive-*.tsv`): synthetic files built by the
  specification side (headers, index tables, string tables, archives with folders, odd names, compressed and
  stored entries, damaged directories); the expected values are the reference implementation's results on them
  (the archive builder's own contents were asserted to match for the well-formed ones).
- **Batch 3 vectors** (`palette.tsv`, `png.tsv`, `wav.tsv`, `sound-bank.tsv`): synthetic pixel data, images
  and sound files built by the specification side; PNG outputs were also opened with an independent PNG
  library and the pixels compared; expected values are the reference implementation's results.
- **Batch 4 vectors** (`gui-set.tsv`, `landscape.tsv`): synthetic dialog-set and landscape-texture files
  built by the specification side (random head values and control bytes, damaged and truncated variants); the
  expected values are the reference implementation's results.
- **Batch 5 vectors** (`map.tsv`, `map-checksum.tsv`): synthetic map files built by the specification side
  (chunk headers encrypted with the cipher, payloads compressed with the specification side's encoder, framing
  errors, damaged payloads, every chunk kind); the expected values are the reference implementation's results.
- **Batch 6 vectors** (`gfx.tsv`): synthetic sprite files (both header forms, raw and run-length images, every
  kind of damage), indexed access and animation tables; expected values are the reference implementation's results.
- **Batch 7 vectors** (`terrain.tsv`, `data/terrain-*.bin`): the constant tables of the reference implementation
  (derived by the specification side from the original program's data, as numbers only) and its results for
  every pair of classes and every tile type / light / fog combination.
- **Generator:** a program of the specification side (fixed random seed `0x5eed1234abcdef01`); it ran against the
  reference implementation and is not part of this repository.
- **Generated:** 2026-10-20.

## File hashes (SHA-256)

```
0109b5da3cbc74d8bc0ab9cae5811d04fae8904ab45f503dfceee5afa244132b archive-find.tsv
b20fb9e3ebdca1f548056d34184136bdd2b2af3e7c80f94562c6d5108b0cbea2 archive-parse.tsv
1226b9fe37b546fe805aefa8c42f96526e94833f0861e7abb51a5fd0aec9383b archive-read.tsv
fb75ff28daccdced9024bf9816ed7717b8c432eccc1b6263a64fc3ffa638bd2f cipher.tsv
71cb35dc1980b8901963edf2d8effed69aa5279280b29806c2232914824a1799 compress.tsv
f655daeb35e9a244d090b65d3b141588f68f70a0c702202729ae9ddc5204c6f1 gfx.tsv
89d7d162cbcd452baad40b8b2494a7cf780e03fdd7e1aeb80d747f4b86ab5b8e gui-set.tsv
35608e73e23f7696a384207baa6ba0f6e4efcf266d4d2f7ba2b119563ca4d55d header.tsv
5df6f34077284f55c13c6e1e5a4f78e35539f8a3c03073c5b5f0b8c666caaeac landscape.tsv
ae51445583fa7e97112cb4f691b83a25d99f6fc3ccf079d1f5d19e535b683438 map-checksum.tsv
a6f6b2d8446c82c7472fe1ddbe45f45f026a8afe06be96a09b71bc918fde14b0 map.tsv
e8c7d9349deac4b0674d576de69173224dffeb5cf0dbbcbe69b603387cd67f43 palette.tsv
58df330518bfa8ece1f19b00326cf0d084ca75434963d2920f29f3d01c4db7d0 png.tsv
6ce9c6b0c6bc2efed488b78547e6f998c77b54a530d90f547d6716730abc5c8b sound-bank.tsv
aa2def45b37a895b1b2f07b68ddacf38d27950ab879c99e46deca862dfb3e6df terrain.tsv
7f24a6b5273b8c8038a76d786ecd30b5dee66d0f9c46d4b56167c244118ecd74 texts-fallback.tsv
20b9660e6a00d60e1b75afe901529c9ae39dba98c90b8d6cba959142eb599eae texts.tsv
7fa7bff13c02d3e13125b31cfcb380d2edac298f8d396e466af0bd4a5bcba132 wav.tsv
0cebee06747ab4a727d1b49e723cd2974b8cfcaf7d9bbb71328afc825eb143c0 data/archive-compressed-and-stored-0.plain
c8c1625661851d9caf1f4b7a242e3b02e95eb1415d4a064261c606db1d736401 data/archive-compressed-and-stored-1.plain
c8c1625661851d9caf1f4b7a242e3b02e95eb1415d4a064261c606db1d736401 data/archive-compressed-and-stored-2.plain
fd14b7df136dc53442297b114cf1384aafff01710312b5d39642a7374afe6223 data/archive-compressed-and-stored.lib
d629137229a7f830bcf5fcf6eb290315d2d69f99e61b649ca742ea188eaad56a data/archive-many-files.lib
3d840243d1ef78dcc5df35ef92e50cef39141a92cfcb8801fb2efb292edff81f data/distance-boundaries.plain
bac2315191a7a3236f93462c297b4680fb3252eaf568e590015065614c633d5a data/distance-boundaries.stream
74a76a93c4c0bf506b928c1bce361a82c8d3aaa8236083f4697d05b5131b13fd data/every-match-length-distance-4.plain
6cd8d0a20d292c71c51506168dbe429413a3188bbeddec580500986952b651b9 data/every-match-length-distance-4.stream
09c1c4039c33ea3b1bde1753e4e9ac2332ead8fc6748f4e8703a3234cc030913 data/full-3836-entries.bin
3eea90fe17ec0027a40091918d319ebbb7a86b675b519ff116415fcc9ef148e6 data/gfx-many-random.gfx
7f435a23b2219cb7075ad13caa9fc3bf1c72bbdad4e7cd292a091bdb206fd323 data/gui-many-controls.bin
69073f6e0e80d9dfbd2190b7cf555f9e46d0cc1c79ce5e3dbb98dc21709e9db4 data/gui-twelve-dialogs.bin
de6275a4f8c5ff60414cd72dba2655de4022d973b51b9023b34110c6eaf54d05 data/landscape-hardware-then-software-to-end.bin
705a4a57e15b30a1fa372dea6d3ba1c4e08b826233f081f957ace6ea5ced79a6 data/landscape-one-256x256-page.bin
94e349abd2d3b10207a6ca4feab67af12a445bc23cbcc9534f9956d53b557f1d data/landscape-one-256x256-page.pixels
36211ab14f3ead52f4b089ecd315dd886ab369ed80d62ecfd4ad5eebdb3d0287 data/landscape-software-only.bin
5696b858ba07b492e98889960c944716b1ad9efdce35391682d6ac1fb4beae59 data/landscape-software-then-hardware-sized.bin
eab68db929676744ab4bd4b4cf7d59b10c6879bcbd245836faffc9466ebb935d data/landscape-software-then-hardware-to-end.bin
f3941083bebe41e2f7108b91b92ccb9fb2c98d9400c31d34383c967fde3efac6 data/landscape-two-128x128-pages.bin
11d0cc0237d9fc9a426f46eeacaa0d73a023aac721af77aecb2c7e6753552fa4 data/landscape-two-128x128-pages.pixels
1e1d4667dc8439ba2360fb0ade01710542be196d903bd8b989a719add0039695 data/long-default.keystream
6504cd7990bce3f75a0eea2c7b25ab501ad0ebf16307660cd5d433235a1c5bc2 data/long-other.keystream
4b4d9db6585f0dd4da579827a8061ffeda33effabd9bbc25e1fd2316f1c0d6fa data/map-big-48x48-old-version-0.plain
612fb77bcb20fd8c0f979a20ab52c0a3db811ca9f54d140080a39f9198e1eeda data/map-big-48x48-old-version-1.plain
8b87e14b1025180cb93670132e2c360e7c5f9f25899ebac4e5a0b2c254c401d2 data/map-big-48x48-old-version.map
2ac57df1e9de06a33e8515eed1a784dfa7422eab7b6c062362af79468fa73427 data/map-full-16x16-5.plain
bcf33d1a0e587dc15352f23620a6dc99b0d4e4f7da6638ffac4c81c8f761ce1e data/map-full-16x16-6.plain
f06fda38e1705b893458aedb54da6fe954cb30cf76cdb0db6ece861cc6437b85 data/map-full-16x16.map
0697ab9a517b7bb8f94f3c0025f1e63c73f0d066e4affcba413bdba30efbd059 data/more-than-3836-entries.bin
946e94c7f49a84f825dc555f8918951d965b8ce741213262ac018aef99514090 data/palette-argb4444.rgba
2cba459f69991dcae93840ba3d638d25bed98296c8145b092052b50f2f85b7c9 data/palette-rgb555.rgba
295bf70b9540a31e95a287eec321413754dc171102fec0b1c24aa6f80b5447e6 data/palette-rgb565.rgba
fc77221fe3e3ef386350bdf26162a11e7971ff910d85cadf7949dfc34f3e6e35 data/png-100x200.png
5df66851ac6a028bff96c8861ee0b68ba4fe5d49ffdc84a4476fb4bff73323f1 data/png-100x200.rgba
034b44b6685a3b303e8ed42d4d9b2ce20d3c46635fad8a27a4fd40cc52b524e2 data/png-1x70000.png
4612556d60a92abe974fe042d61c5d210b33b753ca0e1040d38ce8c72183a716 data/png-1x70000.rgba
06d089436fd25a4e73fe3176e7031b74e8d38b0e8269162030d3047c474b2711 data/png-300x1.png
a4cf42cd29bbb33b8499e6e11472aaabe8ea8c5dffa21146a1a777a62bf912ae data/png-300x1.rgba
327f155809b6620cb1ddca15c88f600dc8974c85c36a8d8d1bc3b42e7d8e0de2 data/png-63x260.png
864a503a816f66c4c6e3934c9ce8b49b4acf7392f5c8896ad23665198e14b2e5 data/png-63x260.rgba
9a902921a3b063a3ee00508f4a788a0eab3157a0fbe5b0804f9607289e42fc84 data/png-64x255-raw-65535.png
268ef10d6bf312c1cd2b522ca9b6df0a35e5b0901aad8058dd9146d69f060eb7 data/png-64x255-raw-65535.rgba
e1955640ff7e6eaf7882cf57fa741f120f01c0ba35e3ccb65a352e891009c8bf data/png-64x256-raw-65792.png
12e49a65eb0f73e6e8c2e7ac4ee1e00bce8a53f172ec8bea0c559163d10a97bf data/png-64x256-raw-65792.rgba
53b5c4a0d0c5aad424b859184a9e81b9f8a7f8715cedc4394d6d1e0de7a01492 data/png-64x510-two-full-blocks.png
8b7d8a818ef5100a3069eb742d1c401ffce063a54722f6c3acb3258c145f084a data/png-64x510-two-full-blocks.rgba
7186aa44a01824ae822350508bb8e47f152f2540fc0430982eb7f8e5abd07fd3 data/png-solid-32x32.png
f995c602e82e5502a543d4834618952e2c477b86b94fb34153db8f010041ff80 data/png-solid-32x32.rgba
1d9fc49f675b0f22674c1434610bab08142cadef1143d3253a1c765a8b7dd5b2 data/random-66000-greedy.plain
46a3eb8e64f776f6e972b9c16eae28e6891a68a529df6aef30fe994fdd8cee8e data/random-66000-greedy.stream
77fcaf5560d90c86fcbea41790c96a45b2c808d60afa688938bb85fab0fcd53c data/rebuild-skewed.plain
f786659d2490f9674144067e383a5cddbe8e75399c154306db13fdd79d3db72c data/rebuild-skewed.stream
1ee8e877eef46e152eda65131661e77a8d3bd0b85053432e47f35200f7a29124 data/rebuild-with-ties.plain
b7f088c5ddef3804e68bedab5ded7a83bc263bc6bc3f39272b1743e5c2f650a0 data/terrain-minimap.bin
1fd8417d0a1953387a4a8c14d51aab54cb83f2c7b5f6c8c7cd9d17f171a6879c data/terrain-transition.bin
c7f1ff7d161d3f666d1da6ada34bba5b9c08a5d8f8009a9fd7b6e0dbc27bdea1 data/text-1000.plain
d2d39e30572224a7eb3cbaf97d651970f47136ccda6bc2ac61ce831fb967d3b5 data/text-20000.plain
54840b7e8a2fe1966646816236d7f3cdca7b01b65663a2def06128f5b14c8b08 data/text-20000.stream
5439bad995708af552b3f47dc8bded0cacf281d9a3ee6640c6ac7d79f18eddb2 data/text-70000.plain
58c12d9fb6994f4245d70bbaac8e94f76436336407a5935df54281a7f4da46f6 data/text-70000.stream
c9ac311b4c3b272e455a81197e8bde1cfb7f3e54894bf3eb68124e29fbb9b678 data/text-with-rebuilds-0.plain
b2e7f2775a417a96312dece0bf4d11d8f54f162b1388daaf7c27dbc380d8aaa6 data/text-with-rebuilds-0.stream
242e4bd43a525067ecb5840dd6e24caf5f8899686a94203c16dbf7a85f363ee3 data/text-with-rebuilds-1.plain
2f75eda8d1ebf01a7eb253258cf790fd32d87d19c06067779db9d32be85f5193 data/text-with-rebuilds-1.stream
3ba235f12d60615a1002041f31bf981f9c2ea35f8235a02c5b06e9e2ea028815 data/text-with-rebuilds-2.plain
5a7fbabff8931125925a890a5ded5383d10039be56b0f640c32e59827958106a data/text-with-rebuilds-2.stream
87b89a4fca445fd8c51f0acfd8a59fb84356b524e035b8331dae1d2379588db1 data/text-with-rebuilds-3.plain
1d9c792af3a030fbb93fa4cf62b9f9a44ff538b1599680094b196c05346501c6 data/text-with-rebuilds-3.stream
ee5ecad9f0e645642bdf29e8ed7e02978a8cdd2ef7ef59b45a704bb09340b4d7 data/text-with-rebuilds-4.plain
fc661ed24824a071985dc9ba1bba39de3ebbd0a389d99b1b1e5faa0036f480b4 data/text-with-rebuilds-4.stream
671691acfeee05365ee7282abab778f33d38cc226784acafe1046b4cef6f9b4a data/text-with-rebuilds-5.plain
bb483a9a5ddf761f3c7fdfe54909b86a51125168071bfa677710585a263cedc4 data/text-with-rebuilds-5.stream
38de5a5b889dfb2b25c33e9558b5d1034d0b23fdd874451eb86cd67f67e30b7f data/two-streams.plain
8f6138ae95a8e9087e6c9ecbd8df7ad26f7049a486c0bf4d6222addbed9a4e61 data/two-streams.stream
f53e6f1aa28ef3f2c3fe091b1c287547ee69a22b22c3e631b52c1f007a9db34e data/wav-adpcm-mono-256.pcm
e91f0c8f1314a9f60da58bf509466efdb0150719c3a293dc4ec7481da86593c7 data/wav-adpcm-mono-36.pcm
e48e4a1ed59a3023d557d778546ab0e438df93a4cf94242387936c0628830931 data/wav-adpcm-mono-512-short-last.pcm
2e9d20095469c05908e9cd893b121b917ece36df3faf3b8cea53b27ba3668568 data/wav-adpcm-saturation.pcm
29bdf889b10bd03c6546c75b5a535a09629d78cb089cbbfe3878695a8be2bded data/wav-adpcm-stereo-512.pcm
ac9e3168fd5ed1c9abf56ab2c1067688c72984669b6aa5eb8ab6a08de255a55c data/wav-adpcm-stereo-512.wav
8f5185e188d9b3f4a6ac3cc1628063d1579de8a563e4c5c253d2801b2a2cd6dc data/wav-adpcm-stereo-72.pcm
d30e8d45b37bf48d46bcb575ea7510aeedc15cf6ec527bc05f4d8356b1fa5d49 data/window-wraparound.plain
c0fe8c634b712ecf37f7789999fd22d7b5c606fea83f2369ac899b88e78e056a data/window-wraparound.stream
```
