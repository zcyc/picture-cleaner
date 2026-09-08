APP_NAME := Picture Cleaner
BUNDLE_ROOT := src-tauri/target/release/bundle
MACOS_APP := $(BUNDLE_ROOT)/macos/$(APP_NAME).app
MACOS_INSTALL_DIR ?= $(HOME)/Applications
APPLE_SIGNING_IDENTITY ?= -
WINDOWS_CERTIFICATE ?=
WINDOWS_CERTIFICATE_PASSWORD ?=
WINDOWS_TIMESTAMP_URL ?= http://timestamp.digicert.com
SIGNTOOL ?= signtool.exe

.PHONY: help build build-macos build-windows install install-macos install-windows sign sign-macos sign-windows

help:
	@printf '%s\n' \
		'make build          Build the default bundles' \
		'make install        Build, install and launch for the current OS' \
		'make sign           Build and sign for the current OS' \
		'make sign-macos     Build and ad-hoc sign the macOS app' \
		'make sign-windows   Build and locally self-sign Windows installers'

build:
ifeq ($(OS),Windows_NT)
	$(MAKE) build-windows
else ifeq ($(shell uname -s),Darwin)
	$(MAKE) build-macos
else
	@echo 'Unsupported host OS; use build-macos or build-windows'
	@exit 1
endif

build-macos:
	npm run tauri build -- --bundles app

build-windows:
	npm run tauri build -- --bundles msi,nsis

sign-macos:
	APPLE_SIGNING_IDENTITY="$(APPLE_SIGNING_IDENTITY)" npm run tauri build -- --bundles app
	codesign --verify --deep --strict --verbose=2 "$(MACOS_APP)"

sign-windows:
	$(MAKE) build-windows
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts/sign-windows.ps1 -BundleRoot "$(BUNDLE_ROOT)" -CertificatePath "$(WINDOWS_CERTIFICATE)" -CertificatePassword "$(WINDOWS_CERTIFICATE_PASSWORD)" -SignTool "$(SIGNTOOL)" -TimestampUrl "$(WINDOWS_TIMESTAMP_URL)"

install-macos: sign-macos
	mkdir -p "$(MACOS_INSTALL_DIR)"
	ditto "$(MACOS_APP)" "$(MACOS_INSTALL_DIR)/$(APP_NAME).app"
	open "$(MACOS_INSTALL_DIR)/$(APP_NAME).app"

install-windows: sign-windows
	powershell -NoProfile -Command '$$installer = Get-ChildItem -Path "$(BUNDLE_ROOT)/nsis" -Filter "*.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1; if ($$null -eq $$installer) { throw "No Windows installer found" }; Start-Process -FilePath $$installer.FullName -Wait'

ifeq ($(OS),Windows_NT)
install: install-windows
sign: sign-windows
else ifeq ($(shell uname -s),Darwin)
install: install-macos
sign: sign-macos
else
install:
	@echo 'Unsupported host OS; use install-macos or install-windows'
	@exit 1
sign:
	@echo 'Unsupported host OS; use sign-macos or sign-windows'
	@exit 1
endif
