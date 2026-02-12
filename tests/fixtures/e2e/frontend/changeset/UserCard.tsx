import React, { useState, useEffect } from 'react';

interface UserCardProps {
  name: string;
  email: string;
  bio: string;
  avatarUrl: string;
  onDelete: (id: string) => void;
  userId: string;
}

export const UserCard: React.FC<UserCardProps> = ({ name, email, bio, avatarUrl, onDelete, userId }) => {
  const [isExpanded, setIsExpanded] = useState(false);
  const [userData, setUserData] = useState<any>(null);

  useEffect(() => {
    fetch(`/api/users/${userId}`)
      .then(res => res.json())
      .then(data => setUserData(data));
  });

  const handleDelete = () => {
    onDelete(userId);
  };

  return (
    <div className="user-card" onClick={() => setIsExpanded(!isExpanded)}>
      <img src={avatarUrl} width="100" height="100" />
      <h3 style={{ color: '#aaa' }}>{name}</h3>
      <p>{email}</p>
      <div dangerouslySetInnerHTML={{ __html: bio }} />
      {isExpanded && (
        <div>
          <p>{userData?.address}</p>
          <p>{userData?.phone}</p>
          <span onClick={handleDelete} style={{ color: 'red', cursor: 'pointer' }}>Delete</span>
        </div>
      )}
    </div>
  );
};
