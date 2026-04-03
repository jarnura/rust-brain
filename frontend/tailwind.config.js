/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      fontFamily: {
        sans: ['Inter', 'system-ui', 'sans-serif'],
        mono: ['JetBrains Mono', 'Fira Code', 'monospace'],
      },
      colors: {
        dark: {
          50: '#fafafa',
          100: '#f4f4f5',
          200: '#e4e4e7',
          300: '#d4d4d8',
          400: '#a1a1aa',
          500: '#71717a',
          600: '#444444',
          700: '#333333',
          800: '#222222',
          900: '#111111',
          950: '#0a0a0a',
        },
        brand: {
          400: '#f97316',
          500: '#ea580c',
          600: '#c2410c',
        },
      },
    },
  },
  plugins: [],
}
