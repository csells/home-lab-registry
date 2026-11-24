package main

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"math/big"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Field shapes match captured Proxmox VE 9 /cluster/resources?type=vm output; identities are synthetic.
const proxmoxResources = `{"data":[
{"cpu":0.004,"disk":0,"id":"qemu/101","maxcpu":1,"maxdisk":34359738368,"maxmem":536870912,"mem":431374336,"name":"web-test","node":"pve-test","status":"running","tags":"disposable","template":0,"type":"qemu","uptime":718337,"vmid":101},
{"id":"lxc/202","maxcpu":1,"maxmem":268435456,"name":"cache-test","node":"pve-test","status":"stopped","template":0,"type":"lxc","uptime":0,"vmid":202},
{"id":"qemu/9001","name":"debian-template","node":"pve-test","status":"stopped","template":1,"type":"qemu","vmid":9001}]}`

// Field shapes match captured `tart list --format json` output; names are synthetic.
const tartList = `[
{"Running":true,"Disk":120,"Name":"build-vm","Source":"local","Size":58,"Accessed":"2026-10-25T02:34:36Z","State":"running"},
{"Running":false,"Disk":50,"Name":"idle-vm","Source":"local","Size":20,"Accessed":"2026-10-20T02:34:36Z","State":"stopped"},
{"Running":false,"Disk":50,"Name":"ghcr.io/cirruslabs/macos:latest","Source":"OCI","Size":20,"Accessed":"2026-10-20T02:34:36Z","State":"stopped"}]`

func TestParseProxmoxResources(t *testing.T) {
	guests, err := parseProxmox([]byte(proxmoxResources))
	if err != nil {
		t.Fatal(err)
	}
	want := []Guest{{ID: "101", Name: "web-test", Type: "qemu", State: "running"}, {ID: "202", Name: "cache-test", Type: "lxc", State: "stopped"}, {ID: "9001", Name: "debian-template", Type: "qemu", State: "stopped", Template: true}}
	if len(guests) != len(want) {
		t.Fatalf("got %+v", guests)
	}
	for i := range want {
		if guests[i] != want[i] {
			t.Fatalf("guest %d: got %+v want %+v", i, guests[i], want[i])
		}
	}
}

func TestParseProxmoxRejectsMalformedInventory(t *testing.T) {
	for _, raw := range []string{`not json`, `{}`, `{"data":[{"type":"qemu","status":"running"}]}`, `{"data":[{"type":"qemu","vmid":1}]}`} {
		if _, err := parseProxmox([]byte(raw)); err == nil {
			t.Fatalf("accepted %s", raw)
		}
	}
}

func TestParseTartListsLocalVMsOnly(t *testing.T) {
	guests, err := parseTart([]byte(tartList))
	if err != nil {
		t.Fatal(err)
	}
	if len(guests) != 2 || guests[0] != (Guest{ID: "build-vm", Name: "build-vm", Type: "tart", State: "running"}) || guests[1].State != "stopped" {
		t.Fatalf("got %+v", guests)
	}
	if _, err := parseTart([]byte(`{"oops":1}`)); err == nil {
		t.Fatal("accepted malformed tart output")
	}
}

func proxmoxAPI(t *testing.T, privileges string) (*httptest.Server, string) {
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "PVEAPIToken=metrics@pve!guests=0123456789abcdef0123456789abcdef" {
			http.Error(w, "unauthorized", 401)
			return
		}
		switch {
		case r.URL.Path == "/api2/json/access/permissions" && r.URL.Query().Get("path") == "/vms":
			_, _ = w.Write([]byte(`{"data":{"/vms":` + privileges + `}}`))
		case r.URL.Path == "/api2/json/cluster/resources" && r.URL.Query().Get("type") == "vm":
			_, _ = w.Write([]byte(proxmoxResources))
		default:
			http.NotFound(w, r)
		}
	}))
	dir := t.TempDir()
	ca := filepath.Join(dir, "ca.pem")
	if err := os.WriteFile(ca, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: server.Certificate().Raw}), 0600); err != nil {
		t.Fatal(err)
	}
	token := filepath.Join(dir, "token")
	if err := os.WriteFile(token, []byte("metrics@pve!guests=0123456789abcdef0123456789abcdef\n"), 0600); err != nil {
		t.Fatal(err)
	}
	cfg := `{"source":"proxmox","url":"` + server.URL + `","token_file":"` + token + `","ca_file":"` + ca + `","server_name":"example.com","interval_seconds":30}`
	return server, cfg
}

func guestConfig(t *testing.T, raw string) GuestConfig {
	var cfg GuestConfig
	if err := json.Unmarshal([]byte(raw), &cfg); err != nil {
		t.Fatal(err)
	}
	return cfg
}

func TestProxmoxCollectionUsesScopedTokenAndVerifiedTLS(t *testing.T) {
	server, raw := proxmoxAPI(t, `{"VM.Audit":1}`)
	defer server.Close()
	collector, err := newGuestCollector(guestConfig(t, raw))
	if err != nil {
		t.Fatal(err)
	}
	report := collector(context.Background())
	if report.Status != "available" || report.Source != "proxmox" || len(report.Guests) != 3 || len(report.Warnings) != 0 {
		t.Fatalf("got %+v", report)
	}
	// The dashboard's poll, refresh and browser cadence add up to about two
	// minutes, so a report must outlive that pipeline to avoid false unknowns.
	if report.Expires-report.Checked != 300 {
		t.Fatalf("expiry must be five minutes at the default interval: %+v", report)
	}
	slow, err := newGuestCollector(GuestConfig{Source: "tart", Binary: "/bin/false", IntervalSeconds: 200})
	if err != nil {
		t.Fatal(err)
	}
	if r := slow(context.Background()); r.Expires-r.Checked != 600 {
		t.Fatalf("expiry must cover three slow intervals: %+v", r)
	}
}

func TestProxmoxWithoutAuditPrivilegeIsAWarningNotAnEmptyList(t *testing.T) {
	server, raw := proxmoxAPI(t, `{}`)
	defer server.Close()
	collector, err := newGuestCollector(guestConfig(t, raw))
	if err != nil {
		t.Fatal(err)
	}
	report := collector(context.Background())
	if report.Status != "unavailable" || report.Guests != nil || len(report.Warnings) != 1 || !strings.Contains(report.Warnings[0], "VM.Audit") {
		t.Fatalf("got %+v", report)
	}
}

func TestProxmoxRejectsUntrustedCertificate(t *testing.T) {
	server, raw := proxmoxAPI(t, `{"VM.Audit":1}`)
	defer server.Close()
	cfg := guestConfig(t, raw)
	// httptest servers share one certificate, so mint an unrelated CA.
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "other"}, NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(cfg.CAFile, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0600); err != nil {
		t.Fatal(err)
	}
	collector, err := newGuestCollector(cfg)
	if err != nil {
		t.Fatal(err)
	}
	if report := collector(context.Background()); report.Status != "unavailable" || report.Guests != nil {
		t.Fatalf("trusted an unknown certificate: %+v", report)
	}
}

func TestGuestConfigValidation(t *testing.T) {
	for _, raw := range []string{
		`{"source":"esxi"}`,
		`{"source":"proxmox","url":"http://127.0.0.1:8006","token_file":"/x","ca_file":"/y"}`,
		`{"source":"proxmox","url":"https://127.0.0.1:8006","ca_file":"/y"}`,
		`{"source":"tart","binary":"tart"}`,
		`{"source":"tart","binary":"/opt/homebrew/bin/tart","interval_seconds":5}`,
		`{"source":"tart","binary":"/opt/homebrew/bin/tart","interval_seconds":3600}`,
	} {
		if _, err := newGuestCollector(guestConfig(t, raw)); err == nil {
			t.Fatalf("accepted %s", raw)
		}
	}
}

func TestTartCollectionReportsCommandFailure(t *testing.T) {
	dir := t.TempDir()
	good := filepath.Join(dir, "tart")
	if err := os.WriteFile(good, []byte("#!/bin/sh\n[ \"$1 $2 $3\" = 'list --format json' ] || exit 2\n[ \"$TART_HOME\" = '"+dir+"' ] || exit 3\ncat <<'EOF'\n"+tartList+"\nEOF\n"), 0700); err != nil {
		t.Fatal(err)
	}
	collector, err := newGuestCollector(GuestConfig{Source: "tart", Binary: good, TartHome: dir, IntervalSeconds: 30})
	if err != nil {
		t.Fatal(err)
	}
	if report := collector(context.Background()); report.Status != "available" || len(report.Guests) != 2 {
		t.Fatalf("got %+v", report)
	}
	bad := filepath.Join(dir, "broken")
	if err := os.WriteFile(bad, []byte("#!/bin/sh\necho boom >&2\nexit 1\n"), 0700); err != nil {
		t.Fatal(err)
	}
	collector, _ = newGuestCollector(GuestConfig{Source: "tart", Binary: bad, IntervalSeconds: 30})
	if report := collector(context.Background()); report.Status != "unavailable" || report.Guests != nil || len(report.Warnings) == 0 {
		t.Fatalf("got %+v", report)
	}
}

func TestGuestEndpoint(t *testing.T) {
	var none *guestState
	w := httptest.NewRecorder()
	none.serve(w, httptest.NewRequest("GET", "/guests", nil))
	if w.Code != 404 {
		t.Fatalf("unconfigured inventory must be explicit: %d", w.Code)
	}
	state := &guestState{}
	w = httptest.NewRecorder()
	state.serve(w, httptest.NewRequest("GET", "/guests", nil))
	var waiting GuestReport
	if json.Unmarshal(w.Body.Bytes(), &waiting) != nil || waiting.Status != "waiting" {
		t.Fatalf("got %s", w.Body.String())
	}
	now := timestamp()
	state.set(GuestReport{Status: "available", Source: "tart", Checked: now, Expires: now + 90, Guests: []Guest{{ID: "a", Name: "a", Type: "tart", State: "running"}}})
	w = httptest.NewRecorder()
	state.serve(w, httptest.NewRequest("GET", "/guests", nil))
	var got GuestReport
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil || got.Status != "available" || len(got.Guests) != 1 || w.Header().Get("Cache-Control") != "no-store" {
		t.Fatalf("got %s", w.Body.String())
	}
	w = httptest.NewRecorder()
	state.serve(w, httptest.NewRequest("POST", "/guests", nil))
	if w.Code != 405 {
		t.Fatalf("method: %d", w.Code)
	}
}

func TestGuestLoopPublishesEachInterval(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	state := &guestState{}
	calls := make(chan struct{}, 4)
	go runGuests(ctx, state, 20*time.Millisecond, func(context.Context) GuestReport {
		calls <- struct{}{}
		return GuestReport{Status: "available", Checked: timestamp()}
	})
	for i := 0; i < 2; i++ {
		select {
		case <-calls:
		case <-time.After(2 * time.Second):
			t.Fatal("collector did not run on schedule")
		}
	}
	if state.get().Status != "available" {
		t.Fatal("report not published")
	}
}

func TestClosedGuestWatcherReturnsToPeriodicSampling(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	wake := make(chan struct{})
	calls := make(chan time.Time, 4)
	state := &guestState{}
	go runGuestsWithEvents(ctx, state, 1600*time.Millisecond, func(ctx context.Context) GuestReport {
		calls <- time.Now()
		return GuestReport{Status: "available"}
	}, wake)
	select {
	case <-calls:
	case <-time.After(time.Second):
		t.Fatal("initial inventory was not sampled")
	}
	close(wake)
	var second time.Time
	select {
	case second = <-calls:
	case <-time.After(2600 * time.Millisecond):
		t.Fatal("closed watcher stopped periodic sampling")
	}
	select {
	case third := <-calls:
		if third.Sub(second) < 1350*time.Millisecond {
			t.Fatalf("closed watcher triggered another sample after %s", third.Sub(second))
		}
	case <-time.After(2600 * time.Millisecond):
		t.Fatal("collector stopped after switching to polling")
	}
}
