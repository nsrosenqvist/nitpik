import React from 'react';

interface UserCardProps {
  name: string;
  email: string;
}

export const UserCard: React.FC<UserCardProps> = ({ name, email }) => {
  return (
    <div className="user-card">
      <h3>{name}</h3>
      <p>{email}</p>
    </div>
  );
};
