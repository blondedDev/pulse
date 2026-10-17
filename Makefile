FFMPEG_VERSION ?= 7.1
FFMPEG_URL ?= https://evermeet.cx/ffmpeg/ffmpeg-$(FFMPEG_VERSION).zip
BIN_DIR = bin
FFMPEG = $(BIN_DIR)/ffmpeg
FIXTURES_DIR = fixtures

FIXTURES = \
	$(FIXTURES_DIR)/sine_440_16_44_mono.wav \
	$(FIXTURES_DIR)/sine_440_16_48_mono.wav \
	$(FIXTURES_DIR)/sine_440_16_96_mono.wav \
	$(FIXTURES_DIR)/sine_440_24_44_mono.wav \
	$(FIXTURES_DIR)/sine_440_32_44_mono.wav \
	$(FIXTURES_DIR)/sine_440_16_44_stereo.wav \
	$(FIXTURES_DIR)/silence_16_44_mono.wav \
	$(FIXTURES_DIR)/1khz_16_44_1.wav \
	$(FIXTURES_DIR)/cover_front.png \
	$(FIXTURES_DIR)/cover_alternate.png \
	$(FIXTURES_DIR)/tagged_basic.flac \
	$(FIXTURES_DIR)/tagged_track_disc_slash.flac \
	$(FIXTURES_DIR)/tagged_with_cover.flac \
	$(FIXTURES_DIR)/tagless.flac \
	$(FIXTURES_DIR)/tagged_mp3.mp3 \
	$(FIXTURES_DIR)/tagged_ogg.ogg \
	$(FIXTURES_DIR)/tagged_opus.opus \
	$(FIXTURES_DIR)/late_start_opus.opus \
	$(FIXTURES_DIR)/tagged_m4a.m4a

.PHONY: all generate clean bin-deps

all: generate

bin-deps: $(FFMPEG)

$(FFMPEG):
	@mkdir -p $(BIN_DIR)
	curl -L -o $(BIN_DIR)/ffmpeg.zip "$(FFMPEG_URL)"
	unzip -o $(BIN_DIR)/ffmpeg.zip -d $(BIN_DIR)
	rm $(BIN_DIR)/ffmpeg.zip
	chmod +x $(FFMPEG)

generate: bin-deps $(FIXTURES)

$(FIXTURES_DIR):
	@mkdir -p $(FIXTURES_DIR)

$(FIXTURES_DIR)/sine_440_16_44_mono.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-acodec pcm_s16le -ar 44100 -ac 1 "$@"

$(FIXTURES_DIR)/sine_440_16_48_mono.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-acodec pcm_s16le -ar 48000 -ac 1 "$@"

$(FIXTURES_DIR)/sine_440_16_96_mono.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-acodec pcm_s16le -ar 96000 -ac 1 "$@"

$(FIXTURES_DIR)/sine_440_24_44_mono.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-acodec pcm_s24le -ar 44100 -ac 1 "$@"

$(FIXTURES_DIR)/sine_440_32_44_mono.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-acodec pcm_s32le -ar 44100 -ac 1 "$@"

$(FIXTURES_DIR)/sine_440_16_44_stereo.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-acodec pcm_s16le -ar 44100 -ac 2 "$@"

$(FIXTURES_DIR)/silence_16_44_mono.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "anullsrc=r=44100:cl=mono" \
		-acodec pcm_s16le -ar 44100 -ac 1 -t 0.5 "$@"

$(FIXTURES_DIR)/1khz_16_44_1.wav: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=1000:duration=2.0" \
		-acodec pcm_s16le -ar 44100 -ac 1 "$@"

# --- Cover art images ---

$(FIXTURES_DIR)/cover_front.png: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "color=c=red:s=16x16:d=0.1" -frames:v 1 -update 1 "$@"

# A second, different image. Cover-replacement tests need art that is not already
# embedded in tagged_with_cover.flac, or "replaced" and "unchanged" look identical.
$(FIXTURES_DIR)/cover_alternate.png: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "color=c=blue:s=8x8:d=0.1" -frames:v 1 -update 1 "$@"

# --- Tagged FLAC with metadata ---

$(FIXTURES_DIR)/tagged_basic.flac: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-metadata title="Test Track" \
		-metadata artist="Test Artist" \
		-metadata ALBUMARTIST="Test Album Artist" \
		-metadata album="Test Album" \
		-metadata track="3" \
		-metadata disc="1" \
		-metadata YEAR="2024" \
		-codec:a flac "$@"

$(FIXTURES_DIR)/tagged_track_disc_slash.flac: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-metadata title="Slash Track" \
		-metadata artist="Slash Artist" \
		-metadata ALBUMARTIST="Slash Album Artist" \
		-metadata album="Slash Album" \
		-metadata track="5/12" \
		-metadata disc="2/3" \
		-metadata YEAR="2023-06-15" \
		-codec:a flac "$@"

# --- FLAC with embedded cover art (CoverFront) ---

$(FIXTURES_DIR)/tagged_with_cover.flac: $(FIXTURES_DIR)/cover_front.png | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-metadata title="Cover Track" \
		-metadata artist="Cover Artist" \
		-metadata ALBUMARTIST="Cover Album Artist" \
		-metadata album="Cover Album" \
		-metadata track="1" \
		-metadata disc="1" \
		-metadata YEAR="2022" \
		-codec:a flac $(FIXTURES_DIR)/.tagged_with_cover_tmp.flac && \
	$(FFMPEG) -y -i $(FIXTURES_DIR)/.tagged_with_cover_tmp.flac \
		-i $(FIXTURES_DIR)/cover_front.png \
		-map 0 -map 1 -c copy -disposition:v:0 attached_pic "$@" && \
	rm -f $(FIXTURES_DIR)/.tagged_with_cover_tmp.flac

# --- Tagless FLAC (no metadata) ---

$(FIXTURES_DIR)/tagless.flac: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" -codec:a flac "$@"

# --- Tagged MP3 ---

$(FIXTURES_DIR)/tagged_mp3.mp3: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-metadata title="MP3 Track" \
		-metadata artist="MP3 Artist" \
		-metadata album="MP3 Album" \
		-metadata track="7" \
		-codec:a libmp3lame "$@"

# --- Tagged OGG ---

$(FIXTURES_DIR)/tagged_ogg.ogg: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-metadata title="OGG Track" \
		-metadata artist="OGG Artist" \
		-metadata album="OGG Album" \
		-metadata track="9" \
		-codec:a libvorbis "$@"

# --- Tagged Opus ---

$(FIXTURES_DIR)/tagged_opus.opus: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=2" -ac 2 \
		-metadata title="Opus Track" \
		-metadata artist="Opus Artist" \
		-metadata album="Opus Album" \
		-metadata track="3" \
		-codec:a libopus "$@"

# --- Late-start Opus ---

$(FIXTURES_DIR)/late_start_opus.opus: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "aevalsrc=0.4*sin(2*PI*(300+600*t)*t)+0.1*random(0):s=48000:d=2" -ac 2 \
		-output_ts_offset 0.013 -codec:a libopus "$@"

# --- Tagged M4A (MP4 ilst) ---

$(FIXTURES_DIR)/tagged_m4a.m4a: | $(FIXTURES_DIR)
	$(FFMPEG) -y -f lavfi -i "sine=frequency=440:duration=0.5" \
		-metadata title="M4A Track" \
		-metadata artist="M4A Artist" \
		-metadata album="M4A Album" \
		-metadata track="4" \
		-codec:a aac "$@"

clean:
	rm -f $(FIXTURES)

# ============================================================
# Test orchestration
# ============================================================
#
# Inner loop:   make test           (seconds)
# Pre-push:     make test-san       (5-15 min, runs sanitizers + miri + careful)
# Pre-release:  make test-full      (same as test-san, kept as a release gate)
#
# Required tooling (one-time setup):
#   rustup toolchain install nightly
#   rustup +nightly component add rust-src miri
#   cargo install cargo-careful
#
# Notes:
#   - Miri does not support FFI, so only the pure-Rust, FFI-free crates listed in
#     MIRI_CRATES are run. Add crates to MIRI_CRATES only after confirming they
#     (and their dependencies) pull in no FFI — e.g. music_library/music_indexer
#     are excluded because rusqlite links a bundled C library.
#   - ASAN and TSAN are mutually exclusive (separate builds, separate processes).
#   - Sanitizer builds rebuild std (-Zbuild-std), so each is slow on first run.
#   - Nightly-based targets (careful/asan/tsan/miri) exclude the gpui crates
#     (pawse, ui_components, audio_engine, ui_resources) because gpui pulls in
#     pathfinder_simd, which enables unstable features on nightly and fails to
#     compile. The audio/library crates listed in SAN_CRATES are where unsafe and
#     concurrency live, so that's what we actually want to sanitize.

SHELL := /bin/bash

TARGET ?= $(shell rustc -vV | sed -n 's/host: //p')

# Crates for the native sanitizer targets (careful/asan/tsan). FFI is fine here
# (these run real machine code); gpui crates are excluded — they fail on nightly.
SAN_CRATES = \
	-p audio_common \
	-p audio_decoder \
	-p audio_output \
	-p cue_parser \
	-p media_integration \
	-p music_indexer \
	-p music_library

# Crates miri can run: pure Rust, no FFI in the crate or its deps. audio_decoder
# is the exception: its Opus decoder is libopus (FFI), so the Opus tests carry
# #[cfg_attr(miri, ignore)]. Its fixture-decoding tests are slow.
MIRI_CRATES = \
	-p audio_common \
	-p cue_parser \
	-p audio_decoder

.PHONY: test test-careful test-asan test-tsan test-miri test-san test-full help-test

test:
	cargo test --workspace

test-careful:
	cargo +nightly careful test $(SAN_CRATES)

test-asan:
	RUSTFLAGS="-Zsanitizer=address" \
	RUSTDOCFLAGS="-Zsanitizer=address" \
	cargo +nightly test $(SAN_CRATES) --target $(TARGET) -Zbuild-std

test-tsan:
	RUSTFLAGS="-Zsanitizer=thread" \
	RUSTDOCFLAGS="-Zsanitizer=thread" \
	cargo +nightly test $(SAN_CRATES) --target $(TARGET) -Zbuild-std

# -Zmiri-disable-isolation lets the fixture-reading tests (cue_parser,
# audio_decoder) perform real file I/O under miri. -Zmiri-deterministic-floats
# turns off miri's deliberate powf/exp jitter, which trips an exact-integer
# assert in symphonia's Vorbis codebook setup.
test-miri:
	MIRIFLAGS="-Zmiri-disable-isolation -Zmiri-deterministic-floats" \
	cargo +nightly miri test $(MIRI_CRATES)

test-san: test-careful test-asan test-tsan test-miri

test-full: test-san

help-test:
	@echo "Test targets (run 'make generate' once first to build fixtures):"
	@echo "  make test          - fast cargo test (inner loop)"
	@echo "  make test-careful  - cargo-careful, extra UB checks in std"
	@echo "  make test-asan     - AddressSanitizer (use-after-free, double-free, OOB)"
	@echo "  make test-tsan     - ThreadSanitizer (data races)"
	@echo "  make test-miri     - Miri (pure-Rust crates only — no FFI)"
	@echo "  make test-san      - careful + asan + tsan + miri (pre-push)"
	@echo "  make test-full     - test-san (pre-release)"
