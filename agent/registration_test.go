package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestAnnouncementCannotRedirectCredential(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { t.Error("credential redirected") }))
	defer target.Close()
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, target.URL, 302) }))
	defer server.Close()
	c := server.Client()
	c.CheckRedirect = func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }
	if postAnnouncement(context.Background(), c, RegistrationConfig{URL: server.URL}, "test-token", announcement{"192.0.2.10", "eth0"}) == nil {
		t.Fatal("redirect accepted")
	}
}
func TestInterfaceMissingIsNotFakeAddress(t *testing.T) {
	if _, err := currentAddress("nonexistent-lab-interface"); err == nil {
		t.Fatal("missing interface accepted")
	}
}
