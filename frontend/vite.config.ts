import { defineConfig } from 'vite';

export default defineConfig({
  server: {
    // 显式绑 IPv4 回环：Node 17+ 把 localhost 解析为 ::1，默认只监听 [::1]，
    // 与后端 127.0.0.1:3001 的口径不一致，浏览器访问 127.0.0.1 会连不上。
    host: '127.0.0.1',
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:3001',
        changeOrigin: true,
      },
    },
  },
});
