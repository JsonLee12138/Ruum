# ruum: build, check, and install.
#
# `make install` puts the binary, a desktop entry and an icon under PREFIX
# (default: ~/.local), so ruum shows up in the Omarchy launcher and in
# "Open with" for directories. The default needs no root:
#
#   make install                         # ~/.local/bin/ruum
#   sudo make install PREFIX=/usr/local  # system-wide
#
# On macOS it installs ruum.app into ~/Applications instead (APPS to
# change that), plus a `ruum` command in BINDIR that runs the app's
# binary, so both Spotlight and a terminal find it.

PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
APPDIR ?= $(PREFIX)/share/applications
ICONDIR ?= $(PREFIX)/share/icons/hicolor/scalable/apps

APPS ?= $(HOME)/Applications
BUNDLE = target/bundle/ruum.app
LSREGISTER = /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
UNAME := $(shell uname -s)

MANIFEST = Cargo.toml
CARGO ?= cargo
TARGET = target/release/ruum
GUI_TARGET = target/release/ruum-gui
ifeq ($(UNAME),Linux)
TARGET = target/portable/ruum
GUI_TARGET = target/portable/ruum-gui
endif
ICON = assets/ruum.svg
DESKTOP = packaging/ruum.desktop.in

# Being on PATH is not enough: an older package can win earlier in it.
define check_install_path
	@resolved=$$(command -v $(1) 2>/dev/null || true); \
	if [ -n "$$resolved" ] && [ "$$resolved" -ef "$(BINDIR)/$(1)" ]; then \
	    :; \
	else \
	    echo; \
	    if [ -n "$$resolved" ]; then \
	        echo "warning: $(1) still resolves to $$resolved"; \
	        echo "         that hides the build in $(BINDIR)"; \
	    else \
	        echo "warning: $(BINDIR) is not on PATH"; \
	    fi; \
	    printf 'Run in your shell: export PATH="%s:$$PATH"\n' "$(BINDIR)"; \
	    echo "Then add that PATH setting to your shell configuration."; \
	fi
endef

.PHONY: help build run install install-scan uninstall bundle bundle-all lint cross-lint test test-ssh ci fmt clean

help:
	@echo "ruum"
	@echo
	@echo "  make build       release build"
	@echo "  make run         build and open the source/root chooser"
	@echo "  make install-scan  build/install the headless SSH helper only"
	@echo "  make install     install to $(PREFIX): binary, desktop entry, icon"
	@echo "                   (macOS: ruum.app into $(APPS))"
	@echo "  make bundle      macOS: build target/bundle/ruum.app and its zip"
	@echo "  make bundle-all  macOS: build .app, .zip, .dmg, and .pkg"
	@echo "  make uninstall   remove what install put there"
	@echo "  make lint        rustfmt --check and clippy -D warnings"
	@echo "  make cross-lint  Windows MSVC clippy cross-check via cargo-xwin"
	@echo "  make test        core and window-harness tests"
	@echo "  make test-ssh    real localhost SSH tests (Linux: bun, openssh)"
	@echo "  make ci          lint, then test"
	@echo "  make fmt         format in place"
	@echo "  make clean       cargo clean"

# Always ask cargo: it is incremental and knows every source file, where a
# make file-target would only compare the binary against the manifest and
# happily install a stale build.
build:
ifeq ($(UNAME),Linux)
	$(CARGO) xtask portable
else
	$(CARGO) build --release -p ruum-app -p ruum-core --bin ruum-gui --bin ruum
endif

# For servers: no desktop, GPU, GPUI build, or elevated permissions needed.
install-scan:
	$(CARGO) build --release -p ruum-scan
	install -d "$(BINDIR)"
	install -m755 target/release/ruum-scan "$(BINDIR)/ruum-scan"
	$(call check_install_path,ruum-scan)

run: build
	$(TARGET)

lint:
	$(CARGO) xtask lint

cross-lint:
	$(CARGO) xtask cross-lint

test:
	$(CARGO) xtask test

# Isolated keys and high localhost port; never uses a real server or root.
test-ssh:
	bun scripts/test-ssh.ts

ci: lint test

fmt:
	$(CARGO) xtask fmt-fix

ifeq ($(UNAME),Darwin)
# Replaced whole rather than copied over: a stale file left inside a signed
# bundle breaks its signature.
install:
	$(CARGO) xtask bundle
	-$(LSREGISTER) -u "$(BUNDLE)" 2>/dev/null || true
	install -d "$(APPS)" "$(BINDIR)"
	rm -rf "$(APPS)/ruum.app"
	ditto "$(BUNDLE)" "$(APPS)/ruum.app"
	touch "$(APPS)/ruum.app" "$(APPS)/ruum.app/Contents/Info.plist"
	$(LSREGISTER) -f "$(APPS)/ruum.app" 2>/dev/null || true
	killall NotificationCenter usernoted 2>/dev/null || true
	if [ -f "$$HOME/Library/LaunchAgents/com.ruum.watchdog.plist" ]; then launchctl kickstart -k "gui/$$(id -u)/com.ruum.watchdog" 2>/dev/null || true; fi
	ln -sf "$(APPS)/ruum.app/Contents/MacOS/ruum" "$(BINDIR)/ruum"
	@echo
	@echo "installed:"
	@echo "  $(APPS)/ruum.app"
	@echo "  $(BINDIR)/ruum -> the app's binary"
	$(call check_install_path,ruum)
	@if [ ! -f "$(APPS)/ruum.app/Contents/Resources/ruum-scan-linux.pack" ]; then \
	    echo; \
	    echo "warning: $(APPS)/ruum.app/Contents/Resources/ruum-scan-linux.pack is missing."; \
	    echo "         Remote SSH auto-deployment requires the Linux musl targets."; \
	    echo "         Run: rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl"; \
	    echo "         Then re-run \`make install\` to embed the remote scanner pack."; \
	fi

uninstall:
	rm -rf "$(APPS)/ruum.app"
	@if [ -L "$(BINDIR)/ruum" ]; then rm -f "$(BINDIR)/ruum"; fi
	@echo "removed"
else
install: build
	install -d $(BINDIR) $(APPDIR) $(ICONDIR)
	install -m755 $(TARGET) $(BINDIR)/ruum
	install -m755 $(GUI_TARGET) $(BINDIR)/ruum-gui
	install -m644 $(ICON) $(ICONDIR)/ruum.svg
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
	    gtk-update-icon-cache -f -t "$(PREFIX)/share/icons/hicolor" 2>/dev/null || true; \
	fi
	VERSION=$$(sed -n 's/^version = "\(.*\)"/\1/p' $(MANIFEST) | head -1) && \
	sed -e 's|@BINDIR@|$(BINDIR)|' -e "s|@VERSION@|$$VERSION|" \
	    $(DESKTOP) > $(APPDIR)/ruum.desktop && \
	chmod 644 $(APPDIR)/ruum.desktop
	@if command -v update-desktop-database >/dev/null 2>&1; then \
	    update-desktop-database $(APPDIR) 2>/dev/null || true; \
	fi
	@echo
	@echo "installed:"
	@echo "  $(BINDIR)/ruum"
	@echo "  $(BINDIR)/ruum-gui"
	@echo "  $(APPDIR)/ruum.desktop"
	@echo "  $(ICONDIR)/ruum.svg"
	@if command -v desktop-file-validate >/dev/null 2>&1; then \
	    desktop-file-validate $(APPDIR)/ruum.desktop || true; \
	fi
	$(call check_install_path,ruum)

uninstall:
	rm -f $(BINDIR)/ruum $(BINDIR)/ruum-gui $(APPDIR)/ruum.desktop $(ICONDIR)/ruum.svg
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
	    gtk-update-icon-cache -f -t "$(PREFIX)/share/icons/hicolor" 2>/dev/null || true; \
	fi
	@if command -v update-desktop-database >/dev/null 2>&1; then \
	    update-desktop-database $(APPDIR) 2>/dev/null || true; \
	fi
	@echo "removed"
endif

bundle:
	$(CARGO) xtask bundle

bundle-all:
	$(CARGO) xtask bundle --all

clean:
	$(CARGO) clean
