package main

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

type RegistrationConfig struct {
	URL       string `json:"url"`
	TokenFile string `json:"token_file"`
	CAFile    string `json:"ca_file,omitempty"`
	Interface string `json:"interface"`
}

type announcement struct {
	Address   string `json:"address"`
	Interface string `json:"interface"`
}

func currentAddress(name string) (announcement, error) {
	i, err := net.InterfaceByName(name)
	if err != nil {
		return announcement{}, err
	}
	if i.Flags&net.FlagUp == 0 {
		return announcement{}, fmt.Errorf("interface down")
	}
	addresses, err := i.Addrs()
	if err != nil {
		return announcement{}, err
	}
	for _, a := range addresses {
		ip, _, e := net.ParseCIDR(a.String())
		if e == nil && ip.To4() != nil && ip.IsGlobalUnicast() {
			return announcement{ip.String(), name}, nil
		}
	}
	return announcement{}, fmt.Errorf("interface has no LAN IPv4 address")
}

func registrationClient(cfg RegistrationConfig) (*http.Client, string, error) {
	u, err := url.Parse(cfg.URL)
	if err != nil || u.Scheme != "https" || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return nil, "", fmt.Errorf("registration requires HTTPS")
	}
	token, err := os.ReadFile(cfg.TokenFile)
	if err != nil {
		return nil, "", err
	}
	secret := strings.TrimSpace(string(token))
	if len(secret) < 32 {
		return nil, "", fmt.Errorf("invalid registration credential")
	}
	roots, err := x509.SystemCertPool()
	if err != nil {
		roots = x509.NewCertPool()
	}
	if cfg.CAFile != "" {
		b, e := os.ReadFile(cfg.CAFile)
		if e != nil {
			return nil, "", e
		}
		if !roots.AppendCertsFromPEM(b) {
			return nil, "", fmt.Errorf("invalid registry CA")
		}
	}
	transport := &http.Transport{TLSClientConfig: &tls.Config{MinVersion: tls.VersionTLS12, RootCAs: roots}, MaxIdleConns: 1, IdleConnTimeout: 150 * time.Second}
	client := &http.Client{Transport: transport, Timeout: 5 * time.Second, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}
	return client, secret, nil
}

func postAnnouncement(ctx context.Context, client *http.Client, cfg RegistrationConfig, token string, value announcement) error {
	body, _ := json.Marshal(value)
	req, err := http.NewRequestWithContext(ctx, "POST", cfg.URL, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("User-Agent", "HomeLabMetrics/1")
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	response, err := client.Do(req)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, 4096))
	if response.StatusCode != 200 {
		return fmt.Errorf("registry HTTP %d", response.StatusCode)
	}
	return nil
}

func registerMachine(ctx context.Context, cfg RegistrationConfig) {
	client, token, err := registrationClient(cfg)
	if err != nil {
		log.Print("registration configuration unavailable; check private config and credential")
		return
	}
	defer client.CloseIdleConnections()
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()
	var last announcement
	var accepted time.Time
	warned := false
	for {
		value, e := currentAddress(cfg.Interface)
		if e == nil && (value != last || time.Since(accepted) >= 120*time.Second) {
			e = postAnnouncement(ctx, client, cfg, token, value)
			if e == nil {
				if value != last {
					log.Printf("registered interface %s address %s", value.Interface, value.Address)
				}
				last = value
				accepted = time.Now()
				if warned {
					log.Print("machine registration recovered")
				}
				warned = false
			}
		}
		if e != nil && !warned {
			log.Print("machine registration unavailable; retrying without discarding local metrics")
			warned = true
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}
