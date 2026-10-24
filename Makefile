.PHONY: all build release windows macos macos-arm64 macos-x86_64 debug clean test check fmt man install

CARGO ?= cargo
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man/man1
BINARY := target/release/cobi
WIN_BINARY := target/x86_64-pc-windows-gnu/release/cobi.exe
MAC_ARM_BINARY := target/aarch64-apple-darwin/release/cobi
MAC_X86_BINARY := target/x86_64-apple-darwin/release/cobi

all: release

release:
	$(CARGO) build --release --offline

windows:
	$(CARGO) build --release --target x86_64-pc-windows-gnu --offline

macos: macos-arm64

macos-arm64:
	$(CARGO) build --release --target aarch64-apple-darwin --offline

macos-x86_64:
	$(CARGO) build --release --target x86_64-apple-darwin --offline

build: release

debug:
	$(CARGO) build --offline

check:
	$(CARGO) check --offline

test:
	$(CARGO) test --offline

fmt:
	$(CARGO) fmt

man:
	man -l man/cobi.1

install: release
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 0755 "$(BINARY)" "$(DESTDIR)$(BINDIR)/cobi"
	install -d "$(DESTDIR)$(MANDIR)"
	install -m 0644 man/cobi.1 "$(DESTDIR)$(MANDIR)/cobi.1"

clean:
	rm -rf target
