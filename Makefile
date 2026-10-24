.PHONY: all build release windows debug clean test check fmt man install

CARGO ?= cargo
PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man/man1
BINARY := target/release/cobi
WIN_BINARY := target/x86_64-pc-windows-gnu/release/cobi.exe

all: release

release:
	$(CARGO) build --release --offline

windows:
	$(CARGO) build --release --target x86_64-pc-windows-gnu --offline

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
