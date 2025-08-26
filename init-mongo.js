db = db.getSiblingDB('dca_bot');
db.createUser({
  user: 'dca_user',
  pwd: 'dca_password',
  roles: [
    {
      role: 'readWrite',
      db: 'dca_bot',
    },
  ],
});

// Create collection with indexes
db.createCollection('dca_purchases');
db.dca_purchases.createIndex({ "timestamp": -1 });
db.dca_purchases.createIndex({ "symbol": 1 });
db.dca_purchases.createIndex({ "order_id": 1 }, { unique: true });