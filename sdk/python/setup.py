from setuptools import setup, find_packages

with open("README.md", encoding="utf-8") as f:
    long_description = f.read()

setup(
    name="orp-client",
    version="0.1.0",
    description="Python SDK for ORP — Object Relationship Platform",
    long_description=long_description,
    long_description_content_type="text/markdown",
    author="Shield of Steel",
    author_email="sentinel@shieldofsteel.com",
    url="https://github.com/shieldofsteel/orp",
    packages=find_packages(exclude=["tests*", "examples*"]),
    python_requires=">=3.8",
    install_requires=[],  # zero required dependencies
    extras_require={
        "realtime": ["websocket-client>=1.0.0"],
        "dev": [
            "pytest>=7.0",
            "pytest-cov",
            "mypy",
            "websocket-client>=1.0.0",
        ],
    },
    classifiers=[
        "Development Status :: 3 - Alpha",
        "Intended Audience :: Developers",
        "License :: OSI Approved :: MIT License",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
        "Topic :: Software Development :: Libraries :: Python Modules",
    ],
    keywords="orp object-relationship-platform maritime tracking sdk",
    project_urls={
        "Bug Reports": "https://github.com/shieldofsteel/orp/issues",
        "Source": "https://github.com/shieldofsteel/orp",
    },
)
