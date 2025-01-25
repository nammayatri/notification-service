const path = require("path");

module.exports = {
  mode: "development",
  entry: "./index.js",
  output: {
    filename: "bundle.js",
    path: path.resolve(__dirname, "dist"),
    publicPath: "/", // Ensures proper serving of files
  },
  // devServer: {
  //   static: {
  //     directory: path.join(__dirname, "dist"), // Serve files from "dist"
  //   },
  //   compress: true, // Enable gzip compression
  //   port: 8080, // Use desired port
  //   open: true, // Automatically open browser
  //   historyApiFallback: true, // Support SPA
  //   headers: {
  //     "Access-Control-Allow-Origin": "*", // Allow CORS
  //     "Access-Control-Allow-Methods": "GET, POST, PUT, DELETE, PATCH, OPTIONS",
  //     "Access-Control-Allow-Headers":
  //       "X-Requested-With, content-type, Authorization",
  //   },
  // },
};
