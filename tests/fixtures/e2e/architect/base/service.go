package service

// UserService handles user operations.
type UserService struct {
	DB *Database
}

// GetUser fetches a user.
func (s *UserService) GetUser(id int) (*User, error) {
	return s.DB.FindUser(id)
}
