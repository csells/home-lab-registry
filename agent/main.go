// lab-metrics keeps bounded local history and optionally announces its LAN address.
package main

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/shirou/gopsutil/v4/cpu"
	"github.com/shirou/gopsutil/v4/disk"
	"github.com/shirou/gopsutil/v4/host"
	"github.com/shirou/gopsutil/v4/mem"
)

type Disk struct {
	Name            string   `json:"name"`
	Used            uint64   `json:"used"`
	Total           uint64   `json:"total"`
	Checked         float64  `json:"checked"`
	MetadataPercent *float64 `json:"metadata_percent,omitempty"`
}
type Sample struct {
	GPU         GPU      `json:"gpu"`
	Status      string   `json:"status"`
	Checked     float64  `json:"checked"`
	Expires     float64  `json:"expires"`
	CPU         *float64 `json:"cpu_percent"`
	MemoryUsed  uint64   `json:"memory_used"`
	MemoryTotal uint64   `json:"memory_total"`
	MemoryNote  string   `json:"memory_note"`
	Disks       []Disk   `json:"disks"`
	Uptime      uint64   `json:"uptime_seconds"`
	Warnings    []string `json:"warnings,omitempty"`
}
type Config struct {
	Registration *RegistrationConfig `json:"registration,omitempty"`
	Listen       string              `json:"listen"`
	Cert         string              `json:"cert"`
	Key          string              `json:"key"`
	ClientCA     string              `json:"client_ca"`
	Paths        []struct {
		Name string `json:"name"`
		Path string `json:"path"`
	} `json:"disks"`
	ThinPools bool `json:"lvm_thin_pools"`
}
type store struct {
	mu       sync.Mutex
	history  []Sample
	capacity int
	changed  chan struct{}
	seq      uint64
	viewers  int
}

func newStore(n int) *store { return &store{capacity: n, changed: make(chan struct{})} }
func (s *store) add(x Sample) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(s.history) == s.capacity {
		s.history = s.history[1:]
	}
	s.history = append(s.history, x)
	s.seq++
	close(s.changed)
	s.changed = make(chan struct{})
}
func (s *store) snapshot() ([]Sample, <-chan struct{}, uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Sample{}, s.history...), s.changed, s.seq
}
func (s *store) viewer(delta int) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if delta > 0 && s.viewers >= 8 {
		return false
	}
	s.viewers += delta
	return true
}
func timestamp() float64 { return float64(time.Now().UnixMilli()) / 1000 }
func parseThinPools(raw []byte, checked float64) ([]Disk, error) {
	var data struct {
		Report []struct {
			LV []struct {
				Name     string `json:"lv_name"`
				VG       string `json:"vg_name"`
				Size     string `json:"lv_size"`
				Percent  string `json:"data_percent"`
				Metadata string `json:"metadata_percent"`
				Type     string `json:"segtype"`
			} `json:"lv"`
		} `json:"report"`
	}
	if err := json.Unmarshal(raw, &data); err != nil {
		return nil, err
	}
	result := []Disk{}
	for _, report := range data.Report {
		for _, lv := range report.LV {
			if lv.Type != "thin-pool" {
				continue
			}
			size, e1 := strconv.ParseFloat(lv.Size, 64)
			pct, e2 := strconv.ParseFloat(lv.Percent, 64)
			meta, e3 := strconv.ParseFloat(lv.Metadata, 64)
			if e1 != nil || e2 != nil || e3 != nil || size <= 0 || pct < 0 || pct > 100 || meta < 0 || meta > 100 {
				return nil, errors.New("invalid thin-pool measurement")
			}
			result = append(result, Disk{Name: lv.VG + "/" + lv.Name, Used: uint64(size * pct / 100), Total: uint64(size), Checked: checked, MetadataPercent: &meta})
		}
	}
	return result, nil
}
func disks(cfg Config) ([]Disk, []string) {
	result := []Disk{}
	warnings := []string{}
	checked := timestamp()
	for _, p := range cfg.Paths {
		d, err := disk.Usage(p.Path)
		if err != nil {
			warnings = append(warnings, "Filesystem measurement unavailable: "+p.Name)
			continue
		}
		result = append(result, Disk{Name: p.Name, Used: d.Used, Total: d.Total, Checked: checked})
	}
	if cfg.ThinPools {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, "/sbin/lvs", "--nohints", "--reportformat", "json", "--units", "b", "--nosuffix", "-o", "lv_name,vg_name,lv_size,data_percent,metadata_percent,segtype")
		var stderr bytes.Buffer
		cmd.Stderr = &stderr
		raw, err := cmd.Output()
		if stderr.Len() > 0 {
			warnings = append(warnings, "LVM reported a warning; inspect collector logs")
			log.Printf("LVM: %s", stderr.String())
		}
		if err == nil {
			var pools []Disk
			pools, err = parseThinPools(raw, checked)
			result = append(result, pools...)
		}
		if err != nil {
			warnings = append(warnings, "LVM thin-pool measurement unavailable")
		}
	}
	return result, warnings
}
func collect(ctx context.Context, s *store, cfg Config) {
	readGPU, closeGPU := newGPUReader()
	defer closeGPU()
	// Prime the CPU delta before emitting a first reading.
	_, _ = cpu.Percent(0, false)
	tick := time.NewTicker(5 * time.Second)
	defer tick.Stop()
	var cachedDisks []Disk
	var diskWarnings []string
	var diskAt time.Time
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
			checked := timestamp()
			x := Sample{Status: "available", Checked: checked, Expires: checked + 20, MemoryNote: "Physical memory in use: total minus OS-available memory (reclaimable cache excluded)."}
			values, err := cpu.Percent(0, false)
			if err != nil || len(values) != 1 {
				x.Status = "unavailable"
				x.Warnings = append(x.Warnings, "CPU measurement unavailable")
			} else {
				x.CPU = &values[0]
			}
			memory, err := mem.VirtualMemory()
			if err != nil {
				x.Status = "unavailable"
				x.Warnings = append(x.Warnings, "Memory measurement unavailable")
			} else {
				x.MemoryTotal = memory.Total
				x.MemoryUsed = memory.Total - memory.Available
			}
			x.Uptime, err = host.Uptime()
			if err != nil {
				x.Warnings = append(x.Warnings, "Uptime measurement unavailable")
			}
			if time.Since(diskAt) >= 60*time.Second {
				cachedDisks, diskWarnings = disks(cfg)
				diskAt = time.Now()
			}
			x.GPU = readGPU(ctx)
			x.Disks = cachedDisks
			x.Warnings = append(x.Warnings, diskWarnings...)
			s.add(x)
		}
	}
}
func (s *store) stream(w http.ResponseWriter, r *http.Request) {
	if !s.viewer(1) {
		http.Error(w, "stream limit", 429)
		return
	}
	defer s.viewer(-1)
	w.Header().Set("Content-Type", "application/x-ndjson")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Accel-Buffering", "no")
	encoder := json.NewEncoder(w)
	controller := http.NewResponseController(w)
	history, changed, seq := s.snapshot()
	send := func(v any) bool {
		_ = controller.SetWriteDeadline(time.Now().Add(10 * time.Second))
		if encoder.Encode(v) != nil {
			return false
		}
		return controller.Flush() == nil
	}
	if !send(map[string]any{"type": "history", "history": history, "interval_seconds": 5, "sequence": seq}) {
		return
	}
	heartbeat := time.NewTicker(5 * time.Second)
	defer heartbeat.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-changed:
			var samples []Sample
			var next uint64
			samples, changed, next = s.snapshot()
			// Send all samples missed by a slow reader, not fabricated interpolation.
			count := int(next - seq)
			if count > len(samples) {
				count = len(samples)
			}
			if count > 0 && !send(map[string]any{"type": "samples", "samples": samples[len(samples)-count:], "sequence": next}) {
				return
			}
			seq = next
		case <-heartbeat.C:
			if !send(map[string]any{"type": "heartbeat"}) {
				return
			}
		}
	}
}
func main() {
	mode := flag.String("mode", "serve", "serve, stream or snapshot")
	socket := flag.String("socket", "/run/lab-metrics/agent.sock", "local Unix socket")
	config := flag.String("config", "/etc/lab-metrics.json", "private host configuration")
	flag.Parse()
	if *mode != "serve" {
		if *mode != "stream" && *mode != "snapshot" {
			log.Fatal("invalid mode")
		}
		client := http.Client{Transport: &http.Transport{DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return (&net.Dialer{}).DialContext(ctx, "unix", *socket)
		}, ResponseHeaderTimeout: 10 * time.Second}}
		response, err := client.Get("http://localhost/" + *mode)
		if err != nil {
			log.Fatal(err)
		}
		defer response.Body.Close()
		if response.StatusCode != 200 {
			log.Fatalf("agent HTTP %d", response.StatusCode)
		}
		if _, err = io.Copy(os.Stdout, response.Body); err != nil {
			os.Exit(1)
		}
		return
	}
	raw, err := os.ReadFile(*config)
	if err != nil {
		log.Fatal(err)
	}
	var cfg Config
	if err = json.Unmarshal(raw, &cfg); err != nil {
		log.Fatal(err)
	}
	if len(cfg.Paths) == 0 {
		log.Fatal("configure at least one filesystem")
	}
	if err = os.MkdirAll(filepath.Dir(*socket), 0700); err != nil {
		log.Fatal(err)
	}
	// Refuse an existing listener; never unlink somebody else's active socket.
	if conn, e := net.DialTimeout("unix", *socket, time.Second); e == nil {
		conn.Close()
		log.Fatal("socket already listening")
	}
	if err = os.Remove(*socket); err != nil && !os.IsNotExist(err) {
		log.Fatal(err)
	}
	listener, err := net.Listen("unix", *socket)
	if err != nil {
		log.Fatal(err)
	}
	defer os.Remove(*socket)
	if err = os.Chmod(*socket, 0600); err != nil {
		log.Fatal(err)
	}
	s := newStore(120)
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	go collect(ctx, s, cfg)
	if cfg.Registration != nil {
		go registerMachine(ctx, *cfg.Registration)
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/stream", s.stream)
	mux.HandleFunc("/snapshot", func(w http.ResponseWriter, r *http.Request) {
		h, _, seq := s.snapshot()
		s.mu.Lock()
		v := s.viewers
		s.mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"history": h, "sequence": seq, "subscribers": v, "interval_seconds": 5})
	})
	server := &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second, IdleTimeout: 15 * time.Second, MaxHeaderBytes: 4096}
	var networkServer *http.Server
	if cfg.Listen != "" {
		ca, err := os.ReadFile(cfg.ClientCA)
		if err != nil {
			log.Fatal(err)
		}
		roots := x509.NewCertPool()
		if !roots.AppendCertsFromPEM(ca) {
			log.Fatal("invalid client CA")
		}
		networkMux := http.NewServeMux()
		networkMux.HandleFunc("/stream", s.stream)
		networkServer = &http.Server{Addr: cfg.Listen, Handler: networkMux, ReadHeaderTimeout: 5 * time.Second, IdleTimeout: 15 * time.Second, MaxHeaderBytes: 4096, TLSConfig: &tls.Config{MinVersion: tls.VersionTLS13, ClientAuth: tls.RequireAndVerifyClientCert, ClientCAs: roots}}
		go func() {
			if err := networkServer.ListenAndServeTLS(cfg.Cert, cfg.Key); err != nil && err != http.ErrServerClosed {
				log.Fatal(err)
			}
		}()
	}
	go func() {
		<-ctx.Done()
		_ = server.Close()
		if networkServer != nil {
			_ = networkServer.Close()
		}
	}()
	log.Print("collector started; 5s samples, 60s disks, 120 samples in RAM; network stream requires client certificate")
	if err = server.Serve(listener); err != nil && err != http.ErrServerClosed {
		log.Fatal(fmt.Errorf("agent server: %w", err))
	}
}
