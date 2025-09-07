#!/bin/bash
# Ultra-minimal MongoDB startup for Termux

echo "🔧 Creating ultra-minimal MongoDB startup..."

# Create the most basic startup script possible
cat > ~/start-mongodb-minimal.sh << 'EOF'
#!/bin/bash
echo "🍃 Starting MongoDB (ultra-minimal)..."
mongod --dbpath ~/mongodb/data
EOF

chmod +x ~/start-mongodb-minimal.sh

echo "✅ Created ultra-minimal MongoDB startup script!"
echo ""
echo "🚀 Try this:"
echo "  ~/start-mongodb-minimal.sh"
echo ""
echo "If this still doesn't work, your Termux MongoDB might be broken."
echo "In that case, use MongoDB Atlas (cloud) instead."
