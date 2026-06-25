# damascus_laundry — operator Makefile
#
# SRE-owned. Targets cover build, run, and the systemd install
# that drives the v2.0 live runbook (Phase 3).

# Repo layout (override on the command line if you build elsewhere):
#   make build                          # default: target/release
#   make install-systemd                # installs dl-app@.service template
#   make install-systemd USER=damascus  # override the system user
REPO_ROOT     ?= $(shell pwd)
BIN           := $(REPO_ROOT)/target/release/dl-app
SYSTEMD_DIR   := /etc/systemd/system
UNIT_SRC      := $(REPO_ROOT)/scripts/systemd/dl-app.service
UNIT_INST     := dl-app@.service
SYSTEMD_USER  ?= damascus

# --- Build ---

.PHONY: build
build:
	cargo build --release -p dl-app

# --- Run (no systemd) ---

.PHONY: run
run: build
	$(BIN) run --feed live --wallet $(REPO_ROOT)/wallet.json

.PHONY: run-dry
run-dry: build
	$(BIN) run --feed live --wallet $(REPO_ROOT)/wallet.json --dry-run-live

# --- Systemd install (the v2.0 Phase 3 deliverable) ---

.PHONY: install-systemd
install-systemd:
	@echo ">> installing $(UNIT_INST) to $(SYSTEMD_DIR)"
	@install -d -m 0755 $(SYSTEMD_DIR)
	@install -m 0644 $(UNIT_SRC) $(SYSTEMD_DIR)/$(UNIT_INST)
	@if ! id -u '$(SYSTEMD_USER)' >/dev/null 2>&1; then \
	    echo ">> creating system user '$(SYSTEMD_USER)' (no shell, no login)"; \
	    useradd --system --home $(REPO_ROOT) --shell /usr/sbin/nologin '$(SYSTEMD_USER)'; \
	    chown -R '$(SYSTEMD_USER)':'$(SYSTEMD_USER)' $(REPO_ROOT); \
	else \
	    echo ">> user '$(SYSTEMD_USER)' already exists"; \
	fi
	@systemctl daemon-reload
	@echo ""
	@echo "Install complete. To enable and start:"
	@echo "  sudo systemctl enable dl-app@$(SYSTEMD_USER).service"
	@echo "  sudo systemctl start  dl-app@$(SYSTEMD_USER).service"
	@echo ""
	@echo "To watch journald:"
	@echo "  journalctl -u dl-app@$(SYSTEMD_USER).service -f"

.PHONY: uninstall-systemd
uninstall-systemd:
	@echo ">> disabling and removing $(UNIT_INST)"
	-systemctl disable --now dl-app@$(SYSTEMD_USER).service || true
	-rm -f $(SYSTEMD_DIR)/$(UNIT_INST)
	@systemctl daemon-reload
	@echo ">> done. (system user '$(SYSTEMD_USER)' left in place — remove manually if desired)"

.PHONY: systemd-status
systemd-status:
	systemctl status dl-app@$(SYSTEMD_USER).service --no-pager

.PHONY: systemd-logs
systemd-logs:
	journalctl -u dl-app@$(SYSTEMD_USER).service -f

# --- Verification (acceptance test) ---

.PHONY: verify-systemd-unit
verify-systemd-unit:
	@echo ">> systemd-analyze verify $(UNIT_SRC)"
	@tmp=$$(mktemp -d); \
	  cp $(UNIT_SRC) $$tmp/$(UNIT_INST) && \
	  systemd-analyze verify $$tmp/$(UNIT_INST) && \
	  echo "OK: unit passes systemd-analyze verify" ; \
	  rm -rf $$tmp

.PHONY: help
help:
	@echo "Targets:"
	@echo "  build                  cargo build --release -p dl-app"
	@echo "  run                    run dl-app in the foreground"
	@echo "  install-systemd        install dl-app@.service + create '$(SYSTEMD_USER)' user"
	@echo "  uninstall-systemd      disable and remove the unit"
	@echo "  systemd-status         systemctl status for the instance"
	@echo "  systemd-logs           tail journald for the instance"
	@echo "  verify-systemd-unit    run systemd-analyze verify on the unit file"
