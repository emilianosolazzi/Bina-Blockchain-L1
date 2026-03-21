/**
 * @title RateVisualizer
 * @notice Provides visualization tools for rate-related metrics
 * @dev Uses D3.js for rendering charts, designed for web dashboards
 */
class RateVisualizer {
  constructor(containerId, options = {}) {
    this.containerId = containerId;
    this.options = Object.assign({
      width: 800,
      height: 400,
      margin: { top: 20, right: 30, bottom: 30, left: 50 },
      thresholds: {
        warning: 70,
        critical: 90
      },
      timeFormat: '%H:%M:%S',
      animationDuration: 300,
      maxDataPoints: 100,
      // New options for improved visualization
      showMovingAverage: false,    // Enable to show smoothed trend line
      movingAverageWindow: 5,      // Window size for moving average calculation
      responsiveResize: true,      // Enable responsive resizing
      darkMode: false,             // Support dark mode for dashboards
      tooltips: true,              // Enable interactive tooltips
      rateUnit: 'ops/sec'          // Customizable rate unit label
    }, options);
    
    // Track window resize events if responsive
    if (this.options.responsiveResize) {
      this.resizeHandler = this.handleResize.bind(this);
      window.addEventListener('resize', this.resizeHandler);
    }
    
    this.data = [];
    this.movingAverages = [];
    this.initialized = false;
  }
  
  /**
   * Handles window resize events for responsive charts
   */
  handleResize() {
    if (!this.initialized) return;
    
    const container = document.getElementById(this.containerId);
    if (!container) return;
    
    // Get new container dimensions
    const containerWidth = container.clientWidth;
    
    // Only update if width changed significantly
    if (Math.abs(containerWidth - this.options.width) > 50) {
      this.options.width = containerWidth || 800;
      // Redraw the chart with new dimensions
      this.redraw();
    }
  }
  
  /**
   * Completely redraws the chart with current data and dimensions
   */
  redraw() {
    if (!this.initialized || !this.svg) return;
    
    // Clear existing elements
    this.svg.selectAll("*").remove();
    
    // Reinitialize with new dimensions
    this.initialized = false;
    this.initialize();
    
    // Redraw with existing data
    if (this.data.length > 0) {
      const lastPoint = this.data[this.data.length - 1];
      this.update(lastPoint);
    }
  }
  
  /**
   * Initializes the visualization components
   */
  initialize() {
    // Require d3.js to be loaded
    if (!window.d3) {
      console.error('D3.js is required for RateVisualizer');
      return;
    }
    
    const container = document.getElementById(this.containerId);
    if (!container) {
      console.error(`Container element #${this.containerId} not found`);
      return;
    }
    
    // Create SVG element
    this.svg = d3.select(container)
      .append('svg')
      .attr('width', this.options.width)
      .attr('height', this.options.height);
    
    // Create chart group
    this.chartGroup = this.svg.append('g')
      .attr('transform', `translate(${this.options.margin.left},${this.options.margin.top})`);
    
    // Define scales
    this.width = this.options.width - this.options.margin.left - this.options.margin.right;
    this.height = this.options.height - this.options.margin.top - this.options.margin.bottom;
    
    this.xScale = d3.scaleTime()
      .range([0, this.width]);
      
    this.yScale = d3.scaleLinear()
      .range([this.height, 0]);
    
    // Create axes
    this.xAxis = this.chartGroup.append('g')
      .attr('class', 'x-axis')
      .attr('transform', `translate(0,${this.height})`);
      
    this.yAxis = this.chartGroup.append('g')
      .attr('class', 'y-axis');
    
    // Create line generator
    this.line = d3.line()
      .x(d => this.xScale(d.timestamp))
      .y(d => this.yScale(d.rate))
      .curve(d3.curveMonotoneX);
    
    // Create path for line
    this.path = this.chartGroup.append('path')
      .attr('class', 'rate-line')
      .attr('fill', 'none')
      .attr('stroke', 'steelblue')
      .attr('stroke-width', 2);
    
    // Add threshold lines
    this.warningLine = this.chartGroup.append('line')
      .attr('class', 'threshold-line warning')
      .attr('stroke', 'orange')
      .attr('stroke-dasharray', '5,5')
      .attr('stroke-width', 1);
      
    this.criticalLine = this.chartGroup.append('line')
      .attr('class', 'threshold-line critical')
      .attr('stroke', 'red')
      .attr('stroke-dasharray', '5,5')
      .attr('stroke-width', 1);
    
    // Add labels
    this.chartGroup.append('text')
      .attr('class', 'x-axis-label')
      .attr('text-anchor', 'middle')
      .attr('transform', `translate(${this.width/2},${this.height + 30})`)
      .text('Time');
      
    this.chartGroup.append('text')
      .attr('class', 'y-axis-label')
      .attr('text-anchor', 'middle')
      .attr('transform', `translate(-35,${this.height/2}) rotate(-90)`)
      .text('Rate');
    
    // Add annotations group
    this.annotationsGroup = this.chartGroup.append('g')
      .attr('class', 'annotations');
    
    // Add styles
    const style = document.createElement('style');
    style.textContent = `
      .rate-line { transition: stroke-width 0.2s ease-in-out; }
      .rate-line:hover { stroke-width: 3px; }
      .threshold-line.warning { opacity: 0.7; }
      .threshold-line.critical { opacity: 0.7; }
      .annotation { font-size: 11px; font-weight: bold; }
      .annotation.warning { fill: orange; }
      .annotation.critical { fill: red; }
    `;
    document.head.appendChild(style);
    
    // Add moving average line if enabled
    if (this.options.showMovingAverage) {
      this.movingAverageLine = d3.line()
        .x(d => this.xScale(d.timestamp))
        .y(d => this.yScale(d.value))
        .curve(d3.curveMonotoneX);
        
      this.maPath = this.chartGroup.append('path')
        .attr('class', 'moving-average-line')
        .attr('fill', 'none')
        .attr('stroke', 'rgba(255, 165, 0, 0.7)') // Orange with transparency
        .attr('stroke-width', 1.5);
    }
    
    // Add tooltips container if enabled
    if (this.options.tooltips) {
      this.tooltip = d3.select('body').append('div')
        .attr('class', 'rate-tooltip')
        .style('position', 'absolute')
        .style('display', 'none')
        .style('background', 'rgba(255, 255, 255, 0.9)')
        .style('border', '1px solid #ddd')
        .style('border-radius', '4px')
        .style('padding', '6px')
        .style('pointer-events', 'none')
        .style('z-index', '10');
        
      // Add interactive overlay for tooltip triggering
      this.chartGroup.append('rect')
        .attr('class', 'overlay')
        .attr('width', this.width)
        .attr('height', this.height)
        .style('opacity', 0)
        .on('mousemove', this.showTooltip.bind(this))
        .on('mouseout', () => this.tooltip.style('display', 'none'));
    }
    
    // Rate unit label
    if (this.options.rateUnit) {
      this.chartGroup.append('text')
        .attr('class', 'rate-unit-label')
        .attr('text-anchor', 'end')
        .attr('x', -5)
        .attr('y', 10)
        .text(this.options.rateUnit);
    }
    
    // Set dark mode if enabled
    if (this.options.darkMode) {
      this.applyDarkMode();
    }
    
    this.initialized = true;
  }
  
  /**
   * Applies dark mode styling
   */
  applyDarkMode() {
    this.svg.style('background', '#2a2a2a');
    
    // Update axis colors
    this.xAxis.select('path').style('stroke', '#aaa');
    this.xAxis.selectAll('text').style('fill', '#aaa');
    this.xAxis.selectAll('line').style('stroke', '#aaa');
    
    this.yAxis.select('path').style('stroke', '#aaa');
    this.yAxis.selectAll('text').style('fill', '#aaa');
    this.yAxis.selectAll('line').style('stroke', '#aaa');
    
    // Update labels
    this.svg.selectAll('text').style('fill', '#ddd');
    
    // Update threshold lines
    this.warningLine.attr('stroke', 'rgba(255, 165, 0, 0.7)');
    this.criticalLine.attr('stroke', 'rgba(255, 60, 60, 0.7)');
    
    // Update main line
    this.path.attr('stroke', '#5eadff');
  }
  
  /**
   * Shows tooltip with detailed information
   */
  showTooltip(event) {
    if (!this.tooltip || this.data.length === 0) return;
    
    const bisect = d3.bisector(d => d.timestamp).left;
    const x0 = this.xScale.invert(d3.pointer(event)[0]);
    const i = bisect(this.data, x0, 1);
    const d0 = this.data[i - 1];
    const d1 = this.data[i] || d0;
    const d = x0 - d0.timestamp > d1.timestamp - x0 ? d1 : d0;
    
    // Format tooltip content
    const timeStr = d.timestamp.toLocaleTimeString();
    const rateStr = d.rate.toFixed(2);
    
    // Get MA value if available
    let maStr = '';
    if (this.options.showMovingAverage && this.movingAverages.length > 0) {
      const maIndex = this.movingAverages.findIndex(ma => ma.timestamp.getTime() === d.timestamp.getTime());
      if (maIndex >= 0) {
        maStr = `<br>Avg (${this.options.movingAverageWindow}): ${this.movingAverages[maIndex].value.toFixed(2)}`;
      }
    }
    
    // Show tooltip
    this.tooltip
      .style('display', 'block')
      .style('left', (d3.pointer(event, document.body)[0] + 10) + 'px')
      .style('top', (d3.pointer(event, document.body)[1] - 30) + 'px')
      .html(`Time: ${timeStr}<br>Rate: ${rateStr} ${this.options.rateUnit}${maStr}`);
  }
  
  /**
   * Updates the visualization with new data
   * @param newDataPoint Object containing timestamp and rate value
   */
  update(newDataPoint) {
    if (!this.initialized) {
      this.initialize();
    }
    
    if (!this.initialized) return;
    
    // Add new data point
    this.data.push(newDataPoint);
    
    // Limit data points for performance
    if (this.data.length > this.options.maxDataPoints) {
      this.data.shift();
    }
    
    // Update scales
    this.xScale.domain(d3.extent(this.data, d => d.timestamp));
    this.yScale.domain([0, d3.max(this.data, d => d.rate) * 1.1]); // Add 10% padding
    
    // Update axes
    this.xAxis.call(d3.axisBottom(this.xScale)
      .tickFormat(d3.timeFormat(this.options.timeFormat)));
    this.yAxis.call(d3.axisLeft(this.yScale));
    
    // Update line
    this.path
      .datum(this.data)
      .attr('d', this.line);
    
    // Update threshold lines
    const warningThreshold = this.options.thresholds.warning;
    const criticalThreshold = this.options.thresholds.critical;
    
    this.warningLine
      .attr('x1', 0)
      .attr('x2', this.width)
      .attr('y1', this.yScale(warningThreshold))
      .attr('y2', this.yScale(warningThreshold));
      
    this.criticalLine
      .attr('x1', 0)
      .attr('x2', this.width)
      .attr('y1', this.yScale(criticalThreshold))
      .attr('y2', this.yScale(criticalThreshold));
    
    // Check for threshold crossings and add annotations
    const latestPoint = newDataPoint;
    if (latestPoint.rate > criticalThreshold) {
      this.addAnnotation(latestPoint, 'CRITICAL: Rate exceeds critical threshold', 'critical');
    } else if (latestPoint.rate > warningThreshold) {
      this.addAnnotation(latestPoint, 'WARNING: Rate exceeds warning threshold', 'warning');
    }
    
    // Calculate moving average if enabled
    if (this.options.showMovingAverage && this.initialized) {
      this.updateMovingAverage();
      
      // Update moving average line
      if (this.movingAverages.length > 0) {
        this.maPath
          .datum(this.movingAverages)
          .attr('d', this.movingAverageLine);
      }
    }
  }
  
  /**
   * Adds an annotation to the chart
   * @param dataPoint The data point to annotate
   * @param text Annotation text
   * @param className CSS class for styling
   */
  addAnnotation(dataPoint, text, className) {
    const x = this.xScale(dataPoint.timestamp);
    const y = this.yScale(dataPoint.rate);
    
    // Add annotation group
    const annotation = this.annotationsGroup.append('g')
      .attr('class', `annotation ${className}`)
      .attr('transform', `translate(${x},${y - 15})`);
    
    // Add annotation text
    annotation.append('text')
      .attr('text-anchor', 'middle')
      .text(text);
    
    // Add connecting line
    annotation.append('line')
      .attr('x1', 0)
      .attr('y1', 5)
      .attr('x2', 0)
      .attr('y2', 15)
      .attr('stroke', className === 'critical' ? 'red' : 'orange');
    
    // Remove annotation after delay
    setTimeout(() => {
      annotation.remove();
    }, 5000);
  }
  
  /**
   * Updates threshold values
   * @param warningThreshold New warning threshold
   * @param criticalThreshold New critical threshold
   */
  updateThresholds(warningThreshold, criticalThreshold) {
    this.options.thresholds.warning = warningThreshold;
    this.options.thresholds.critical = criticalThreshold;
    
    // Update visualization if initialized
    if (this.initialized && this.data.length > 0) {
      this.update(this.data[this.data.length - 1]);
    }
  }
  
  /**
   * Renders forecasted rates
   * @param forecastData Array of forecasted rate values
   */
  renderForecast(forecastData) {
    if (!this.initialized || forecastData.length === 0) return;
    
    // Remove existing forecast line
    this.chartGroup.selectAll('.forecast-line').remove();
    
    // Create forecast data points
    const lastTimestamp = this.data[this.data.length - 1].timestamp;
    const timeInterval = 60 * 1000; // 1 minute intervals
    
    const forecasts = forecastData.map((rate, i) => ({
      timestamp: new Date(lastTimestamp.getTime() + (i + 1) * timeInterval),
      rate: rate
    }));
    
    // Create forecast line
    const forecastLine = d3.line()
      .x(d => this.xScale(d.timestamp))
      .y(d => this.yScale(d.rate))
      .curve(d3.curveMonotoneX);
    
    // Extend x-scale domain to include forecasts
    const allData = this.data.concat(forecasts);
    this.xScale.domain(d3.extent(allData, d => d.timestamp));
    
    // Update x-axis
    this.xAxis.call(d3.axisBottom(this.xScale)
      .tickFormat(d3.timeFormat(this.options.timeFormat)));
    
    // Draw forecast line
    this.chartGroup.append('path')
      .datum(forecasts)
      .attr('class', 'forecast-line')
      .attr('fill', 'none')
      .attr('stroke', 'purple')
      .attr('stroke-dasharray', '3,3')
      .attr('stroke-width', 2)
      .attr('d', forecastLine);
    
    // Add forecast label
    this.chartGroup.append('text')
      .attr('class', 'forecast-label')
      .attr('x', this.xScale(forecasts[forecasts.length - 1].timestamp))
      .attr('y', this.yScale(forecasts[forecasts.length - 1].rate) - 10)
      .attr('text-anchor', 'middle')
      .attr('fill', 'purple')
      .text('Forecast');
  }
  
  /**
   * Calculates moving average from data points
   */
  updateMovingAverage() {
    const window = this.options.movingAverageWindow;
    if (window < 2 || this.data.length < window) return;
    
    this.movingAverages = [];
    
    // Calculate simple moving average
    for (let i = window - 1; i < this.data.length; i++) {
      let sum = 0;
      for (let j = 0; j < window; j++) {
        sum += this.data[i - j].rate;
      }
      
      this.movingAverages.push({
        timestamp: this.data[i].timestamp,
        value: sum / window
      });
    }
  }
  
  /**
   * Cleans up resources when component is no longer needed
   */
  destroy() {
    if (this.options.responsiveResize && this.resizeHandler) {
      window.removeEventListener('resize', this.resizeHandler);
    }
    
    if (this.tooltip) {
      this.tooltip.remove();
    }
  }

  // Export for module systems
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = RateVisualizer;
  }
}
