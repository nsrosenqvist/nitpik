import React, { useState, useEffect } from 'react';

export const App: React.FC = () => {
  const [data, setData] = useState<any>(null);

  useEffect(() => {
    fetch('/api/data')
      .then(r => r.json())
      .then(d => setData(d));
  });

  return (
    <div>
      <img src={data?.imageUrl} />
      <div dangerouslySetInnerHTML={{ __html: data?.content }} />
    </div>
  );
};
