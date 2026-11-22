package main

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Guest is one hypervisor guest as reported by the local hypervisor. ID is the
// Proxmox VMID or the Tart VM name, matching the registry's guest identities.
type Guest struct {
	ID       string `json:"id"`
	Name     string `json:"name"`
	Type     string `json:"type"`
	State    string `json:"state"`
	Template bool   `json:"template,omitempty"`
}

// GuestReport is a bounded-lifetime observation. Guests is nil, not empty,
// whenever the inventory could not be read.
type GuestReport struct {
	Status   string           `json:"status"`
	Source   string           `json:"source,omitempty"`
	Checked  float64          `json:"checked,omitempty"`
	Expires  float64          `json:"expires,omitempty"`
	Guests   []Guest          `json:"guests"`
	Warnings []string         `json:"warnings,omitempty"`
	Hardware *hardwareReading `json:"hardware,omitempty"`
}

// GuestConfig selects the local inventory. Proxmox uses the local API with a
// read-only token (PVEAuditor on /vms); Tart runs `tart list` as the service user.
type GuestConfig struct {
	Source              string `json:"source"`
	URL                 string `json:"url,omitempty"`
	TokenFile           string `json:"token_file,omitempty"`
	CAFile              string `json:"ca_file,omitempty"`
	ServerName          string `json:"server_name,omitempty"`
	Binary              string `json:"binary,omitempty"`
	TartHome            string `json:"tart_home,omitempty"`
	TartWatchEvents     bool   `json:"tart_watch_events,omitempty"`
	IntervalSeconds     int    `json:"interval_seconds,omitempty"`
	TaskEventsDirectory string `json:"task_events_directory,omitempty"`
}

func (c GuestConfig) interval() time.Duration {
	if c.IntervalSeconds == 0 {
		return 30 * time.Second
	}
	return time.Duration(c.IntervalSeconds) * time.Second
}

const maxInventory = 4 << 20

func parseProxmox(raw []byte) ([]Guest, error) {
	var body struct {
		Data *[]struct {
			VMID     *int    `json:"vmid"`
			Name     string  `json:"name"`
			Type     string  `json:"type"`
			Status   *string `json:"status"`
			Template int     `json:"template"`
		} `json:"data"`
	}
	if err := json.Unmarshal(raw, &body); err != nil {
		return nil, err
	}
	if body.Data == nil {
		return nil, errors.New("missing data")
	}
	guests := []Guest{}
	for _, v := range *body.Data {
		if v.Type != "qemu" && v.Type != "lxc" {
			continue
		}
		if v.VMID == nil || v.Status == nil || *v.Status == "" {
			return nil, errors.New("guest without identity or status")
		}
		guests = append(guests, Guest{ID: strconv.Itoa(*v.VMID), Name: v.Name, Type: v.Type, State: *v.Status, Template: v.Template == 1})
	}
	return guests, nil
}

func parseTart(raw []byte) ([]Guest, error) {
	var vms []struct {
		Name    string `json:"Name"`
		Source  string `json:"Source"`
		Running *bool  `json:"Running"`
	}
	if err := json.Unmarshal(raw, &vms); err != nil {
		return nil, err
	}
	guests := []Guest{}
	for _, v := range vms {
		// OCI entries are cached images, not VMs.
		if v.Source != "local" {
			continue
		}
		if v.Name == "" || v.Running == nil {
			return nil, errors.New("tart VM without identity or state")
		}
		state := "stopped"
		if *v.Running {
			state = "running"
		}
		guests = append(guests, Guest{ID: v.Name, Name: v.Name, Type: "tart", State: state})
	}
	return guests, nil
}

type guestCollector func(context.Context) GuestReport

func newGuestCollector(cfg GuestConfig) (guestCollector, error) {
	if cfg.TartWatchEvents && cfg.Source != "tart" {
		return nil, errors.New("Tart events require the Tart provider")
	}
	if cfg.TaskEventsDirectory != "" {
		if cfg.Source != "proxmox" {
			return nil, errors.New("guest task events require the Proxmox provider")
		}
		if err := validateTaskDirectory(cfg.TaskEventsDirectory); err != nil {
			return nil, err
		}
	}
	if cfg.IntervalSeconds != 0 && (cfg.IntervalSeconds < 10 || cfg.IntervalSeconds > 300) {
		return nil, errors.New("guest interval must be 10-300 seconds")
	}
	// Outlive both missed collections and the dashboard's display pipeline.
	lifetime := max(3*cfg.interval().Seconds(), 300)
	hardware := hardwareCollector()
	report := func(source string, guests []Guest, err error) GuestReport {
		checked := timestamp()
		r := GuestReport{Status: "available", Source: source, Checked: checked, Expires: checked + lifetime, Guests: guests, Hardware: hardware()}
		if err != nil {
			log.Printf("guest inventory: %v", err)
			r.Status, r.Guests, r.Warnings = "unavailable", nil, []string{err.Error()}
		}
		return r
	}
	switch cfg.Source {
	case "proxmox":
		read, err := proxmoxReader(cfg)
		if err != nil {
			return nil, err
		}
		return func(ctx context.Context) GuestReport {
			guests, err := read(ctx)
			return report("proxmox", guests, err)
		}, nil
	case "tart":
		if !filepath.IsAbs(cfg.Binary) {
			return nil, errors.New("tart binary must be an absolute path")
		}
		return func(ctx context.Context) GuestReport {
			ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, cfg.Binary, "list", "--format", "json")
			if cfg.TartHome != "" {
				cmd.Env = append(os.Environ(), "TART_HOME="+cfg.TartHome)
			}
			var stderr bytes.Buffer
			cmd.Stderr = &stderr
			raw, err := cmd.Output()
			if err != nil {
				log.Printf("tart list: %s", strings.TrimSpace(stderr.String()))
				return report("tart", nil, errors.New("Tart inventory unavailable: tart list failed"))
			}
			guests, err := parseTart(raw)
			if err != nil {
				err = fmt.Errorf("Tart inventory unavailable: %w", err)
			}
			return report("tart", guests, err)
		}, nil
	}
	return nil, errors.New("guest source must be proxmox or tart")
}

func proxmoxReader(cfg GuestConfig) (func(context.Context) ([]Guest, error), error) {
	base, err := url.Parse(cfg.URL)
	if err != nil || base.Scheme != "https" || base.Host == "" || base.User != nil || (base.Path != "" && base.Path != "/") || base.RawQuery != "" {
		return nil, errors.New("Proxmox URL must be an HTTPS origin")
	}
	if cfg.TokenFile == "" || cfg.CAFile == "" {
		return nil, errors.New("Proxmox inventory requires token_file and ca_file")
	}
	client := func() (*http.Client, string, error) {
		// Re-read each time so rotated tokens and renewed node certificates apply without restarts.
		raw, err := os.ReadFile(cfg.TokenFile)
		if err != nil {
			return nil, "", errors.New("Proxmox token unreadable")
		}
		token := strings.TrimSpace(string(raw))
		if !strings.Contains(token, "!") || !strings.Contains(token, "=") || strings.ContainsAny(token, "\r\n") {
			return nil, "", errors.New("Proxmox token must be USER@REALM!ID=SECRET")
		}
		ca, err := os.ReadFile(cfg.CAFile)
		if err != nil {
			return nil, "", errors.New("Proxmox CA unreadable")
		}
		roots := x509.NewCertPool()
		if !roots.AppendCertsFromPEM(ca) {
			return nil, "", errors.New("Proxmox CA invalid")
		}
		transport := &http.Transport{Proxy: nil, TLSClientConfig: &tls.Config{MinVersion: tls.VersionTLS12, RootCAs: roots, ServerName: cfg.ServerName}}
		return &http.Client{Transport: transport, Timeout: 10 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}, token, nil
	}
	get := func(ctx context.Context, c *http.Client, token, path string) ([]byte, error) {
		req, err := http.NewRequestWithContext(ctx, "GET", strings.TrimSuffix(base.String(), "/")+path, nil)
		if err != nil {
			return nil, err
		}
		req.Header.Set("Authorization", "PVEAPIToken="+token)
		req.Header.Set("User-Agent", "HomeLabMetrics/1")
		res, err := c.Do(req)
		if err != nil {
			return nil, errors.New("Proxmox API unreachable or certificate not trusted")
		}
		defer res.Body.Close()
		body, err := io.ReadAll(io.LimitReader(res.Body, maxInventory+1))
		if err != nil || len(body) > maxInventory {
			return nil, errors.New("Proxmox API response invalid")
		}
		if res.StatusCode != 200 {
			return nil, fmt.Errorf("Proxmox API HTTP %d", res.StatusCode)
		}
		return body, nil
	}
	return func(ctx context.Context) ([]Guest, error) {
		c, token, err := client()
		if err != nil {
			return nil, fmt.Errorf("Guest inventory unavailable: %w", err)
		}
		defer c.CloseIdleConnections()
		// The resources call silently omits guests the token cannot see, so an
		// empty list proves nothing without the audit privilege.
		raw, err := get(ctx, c, token, "/api2/json/access/permissions?path=%2Fvms")
		if err != nil {
			return nil, fmt.Errorf("Guest inventory unavailable: %w", err)
		}
		var perms struct {
			Data map[string]map[string]int `json:"data"`
		}
		if json.Unmarshal(raw, &perms) != nil || perms.Data["/vms"]["VM.Audit"] != 1 {
			return nil, errors.New("Guest inventory unavailable: token lacks VM.Audit on /vms")
		}
		raw, err = get(ctx, c, token, "/api2/json/cluster/resources?type=vm")
		if err != nil {
			return nil, fmt.Errorf("Guest inventory unavailable: %w", err)
		}
		guests, err := parseProxmox(raw)
		if err != nil {
			return nil, fmt.Errorf("Guest inventory unavailable: %w", err)
		}
		return guests, nil
	}, nil
}

type guestState struct {
	mu      sync.Mutex
	report  GuestReport
	changed chan struct{}
}

func (s *guestState) set(r GuestReport) {
	s.mu.Lock()
	defer s.mu.Unlock()
	different := s.report.Status != r.Status || s.report.Source != r.Source || !reflect.DeepEqual(s.report.Guests, r.Guests) || !reflect.DeepEqual(s.report.Warnings, r.Warnings) || !reflect.DeepEqual(s.report.Hardware, r.Hardware)
	s.report = r
	if s.changed == nil {
		s.changed = make(chan struct{})
	}
	if different {
		close(s.changed)
		s.changed = make(chan struct{})
	}
}
func (s *guestState) get() GuestReport {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.report
	if r.Status == "" {
		r.Status = "waiting"
	}
	return r
}

func (s *guestState) serve(w http.ResponseWriter, r *http.Request) {
	if s == nil {
		http.Error(w, "guest inventory not configured", 404)
		return
	}
	if r.Method != "GET" {
		http.Error(w, "method not allowed", 405)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(s.get())
}

func runGuests(ctx context.Context, s *guestState, interval time.Duration, collect guestCollector) {
	runGuestsWithEvents(ctx, s, interval, collect, nil)
}

func runGuestsWithEvents(ctx context.Context, s *guestState, interval time.Duration, collect guestCollector, wake <-chan struct{}) {
	tick := time.NewTicker(interval)
	defer tick.Stop()
	for {
		s.set(collect(ctx))
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		case <-wake:
			select {
			case <-ctx.Done():
				return
			case <-time.After(time.Second):
			}
			// The wake channel is bounded and already coalesces busy task bursts.
			select {
			case <-wake:
			default:
			}
		}
	}
}

// stream delivers a cached initial report, content changes, and low-frequency
// freshness heartbeats. It does not trigger hypervisor reads per subscriber.
func (s *guestState) stream(w http.ResponseWriter, r *http.Request) {
	if s == nil {
		http.Error(w, "guest inventory not configured", 404)
		return
	}
	if r.Method != "GET" {
		http.Error(w, "method not allowed", 405)
		return
	}
	f, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unavailable", 500)
		return
	}
	w.Header().Set("Content-Type", "application/x-ndjson")
	w.Header().Set("Cache-Control", "no-store")
	tick := time.NewTicker(60 * time.Second)
	defer tick.Stop()
	for {
		s.mu.Lock()
		if s.changed == nil {
			s.changed = make(chan struct{})
		}
		report, changed := s.report, s.changed
		s.mu.Unlock()
		if report.Status == "" {
			report.Status = "waiting"
		}
		controller := http.NewResponseController(w)
		_ = controller.SetWriteDeadline(time.Now().Add(10 * time.Second))
		if err := json.NewEncoder(w).Encode(report); err != nil {
			return
		}
		f.Flush()
		select {
		case <-r.Context().Done():
			return
		case <-changed:
		case <-tick.C:
		}
	}
}
