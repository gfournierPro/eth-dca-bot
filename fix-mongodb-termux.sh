#!/bin/bash
# Fix MongoDB startup for Termux

echo "🔧 Fixing MongoDB configuration for Termux..."

# Check MongoDB version
echo "📋 Checking MongoDB version..."
mongod --version

# Create a simple MongoDB startup script with minimal options
echo "🚀 Creating minimal MongoDB startup script..."
cat > ~/start-mongodb-simple.sh << 'EOF'
#!/bin/bash
echo "🍃 Starting MongoDB with minimal configuration..."
mongod --dbpath ~/mongodb/data --port 27017 --bind_ip 127.0.0.1
EOF

chmod +x ~/start-mongodb-simple.sh

# Create an alternative with basic logging only
echo "🚀 Creating alternative MongoDB startup script..."
cat > ~/start-mongodb-alt.sh << 'EOF'
#!/bin/bash
echo "🍃 Starting MongoDB with basic logging..."
mongod \
  --dbpath ~/mongodb/data \
  --logpath ~/mongodb/logs/mongod.log \
  --port 27017 \
  --bind_ip 127.0.0.1
EOF

chmod +x ~/start-mongodb-alt.sh

# Create a version detector script
cat > ~/test-mongodb.sh << 'EOF'
#!/bin/bash
echo "🧪 Testing MongoDB configurations..."

echo "Testing minimal configuration..."
if timeout 5 mongod --dbpath ~/mongodb/data --port 27017 --bind_ip 127.0.0.1 --help > /dev/null 2>&1; then
    echo "✅ Minimal configuration works"
    echo "Use: ~/start-mongodb-simple.sh"
else
    echo "❌ Minimal configuration failed"
fi

echo ""
echo "Testing with basic logging..."
if timeout 5 mongod --dbpath ~/mongodb/data --port 27017 --bind_ip 127.0.0.1 --logpath ~/mongodb/logs/mongod.log --help > /dev/null 2>&1; then
    echo "✅ Configuration with basic logging works"
    echo "Use: ~/start-mongodb-alt.sh"
else
    echo "❌ Configuration with basic logging failed"
fi

echo ""
echo "Available MongoDB options:"
mongod --help | grep -E "(journal|storage|log)"
EOF

chmod +x ~/test-mongodb.sh

echo "✅ Created alternative MongoDB startup scripts!"
echo ""
echo "📋 Try these in order:"
echo "1. Test configurations: ~/test-mongodb.sh"
echo "2. Try minimal: ~/start-mongodb-simple.sh"
echo "3. Try alternative: ~/start-mongodb-alt.sh"
echo "4. If all fail, use cloud MongoDB Atlas instead"
