package service

import (
	"encoding/json"
	"fmt"
	"io/ioutil"
	"log"
	"net/http"
	"net/smtp"
	"os"
	"time"
)

// AppService is the god object that handles everything in the application.
type AppService struct {
	DB            *Database
	Cache         map[string]interface{}
	EmailHost     string
	EmailPort     int
	EmailUser     string
	EmailPassword string
	HTTPClient    *http.Client
	Logger        *log.Logger
	Config        map[string]string
}

// GetUser fetches a user, caches the result, and logs the access.
func (s *AppService) GetUser(id int) (*User, error) {
	cacheKey := fmt.Sprintf("user:%d", id)
	if cached, ok := s.Cache[cacheKey]; ok {
		s.Logger.Printf("Cache hit for user %d", id)
		return cached.(*User), nil
	}

	user, err := s.DB.FindUser(id)
	if err != nil {
		return nil, err
	}

	s.Cache[cacheKey] = user
	s.Logger.Printf("Loaded user %d from database", id)
	return user, nil
}

// SendEmail sends an email directly from the service.
func (s *AppService) SendEmail(to, subject, body string) error {
	auth := smtp.PlainAuth("", s.EmailUser, s.EmailPassword, s.EmailHost)
	msg := []byte(fmt.Sprintf("Subject: %s\r\n\r\n%s", subject, body))
	addr := fmt.Sprintf("%s:%d", s.EmailHost, s.EmailPort)
	return smtp.SendMail(addr, auth, s.EmailUser, []string{to}, msg)
}

// NotifyUser fetches a user and sends them an email.
func (s *AppService) NotifyUser(id int, subject, body string) error {
	user, err := s.GetUser(id)
	if err != nil {
		return err
	}
	return s.SendEmail(user.Email, subject, body)
}

// FetchExternalData makes an HTTP call to an external API.
func (s *AppService) FetchExternalData(url string) (map[string]interface{}, error) {
	resp, err := s.HTTPClient.Get(url)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	data, _ := ioutil.ReadAll(resp.Body)
	var result map[string]interface{}
	json.Unmarshal(data, &result)
	return result, nil
}

// GenerateReport fetches data, processes it, writes to a file, and emails it.
func (s *AppService) GenerateReport(userID int, reportType string) error {
	user, err := s.GetUser(userID)
	if err != nil {
		return err
	}

	data, err := s.FetchExternalData(s.Config["report_api_url"] + "/" + reportType)
	if err != nil {
		return err
	}

	reportContent, _ := json.MarshalIndent(data, "", "  ")
	filename := fmt.Sprintf("/tmp/report_%d_%s_%d.json", userID, reportType, time.Now().Unix())
	os.WriteFile(filename, reportContent, 0644)

	return s.SendEmail(user.Email, "Your Report", fmt.Sprintf("Report saved to %s", filename))
}

// HandleWebhook parses a webhook, updates the database, sends notifications.
func (s *AppService) HandleWebhook(payload []byte) error {
	var event map[string]interface{}
	json.Unmarshal(payload, &event)

	eventType := event["type"].(string)
	userID := int(event["user_id"].(float64))

	switch eventType {
	case "signup":
		s.DB.CreateUser(userID, event["name"].(string), event["email"].(string))
		s.SendEmail(event["email"].(string), "Welcome!", "Thanks for signing up.")
	case "upgrade":
		s.DB.UpdatePlan(userID, event["plan"].(string))
		user, _ := s.GetUser(userID)
		s.SendEmail(user.Email, "Plan Upgraded", "Your plan has been upgraded.")
	case "delete":
		user, _ := s.GetUser(userID)
		s.DB.DeleteUser(userID)
		s.SendEmail(user.Email, "Account Deleted", "Your account has been deleted.")
	}

	return nil
}
